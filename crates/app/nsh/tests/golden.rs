//! 界面 golden：三张基线逐像素守着“改原语不动页面”。
//!
//! 为什么不是 `cargo test` 的常规项：它要真桌面（拉起 exe、等窗口、抓客户区）。
//! 所以标 `#[ignore]`，由 `cargo test -p envboard-nsh --test golden -- --ignored`
//! 或 `UPDATE_GOLDEN=1` 那一轮显式触发；CI 不跑。
//!
//! 基线口径（写下来免得以后争论）：
//! - 进程以 `NG_GOLDEN=1` 启动：不启用 Mica、不带系统投影，抓图才逐像素稳定；
//! - 窗口固定 1280×840 逻辑点，本机 125% → 1618×1060 像素。**换 DPI 或改窗口尺寸
//!   必须重拍**，尺寸不符会直接失败，不会悄悄放过；
//! - 抓的是整窗（含 DWM 边框与圆角外黑边），所以边框变化也会被抓到；
//! - 基线是 `tests/golden/<case>.png`（RGBA8）。裸 BGRA 一张 4.4 MB，三张不进库。

#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

// ---- 用例 -----------------------------------------------------------------

/// 每个用例 = 名字 + 启动该页要设的环境变量（都带 `NG_GOLDEN=1`）。
const CASES: [(&str, &[(&str, &str)]); 3] = [
    ("env-light", &[("NG_PAGE", "env")]),
    ("env-dark", &[("NG_PAGE", "env"), ("NG_DARK", "1")]),
    ("drawer-light", &[("NG_PAGE", "env"), ("NG_DRAWER", "1")]),
];

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

// ---- 子进程 ---------------------------------------------------------------

/// 测试失败时也要把窗关掉，否则下一次跑会撞上同名进程。
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch(envs: &[(&str, &str)]) -> ChildGuard {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_envboard-nsh"));
    cmd.env("NG_GOLDEN", "1")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    ChildGuard(cmd.spawn().expect("envboard-nsh 起不来"))
}

// ---- Win32 ---------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 3],
}

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lp: isize) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn GetWindowRect(hwnd: isize, out: *mut Rect) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, len: i32) -> i32;
    fn PrintWindow(hwnd: isize, hdc: isize, flags: u32) -> i32;
    fn GetWindowDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn SetProcessDpiAwarenessContext(ctx: isize) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: isize) -> isize;
    fn CreateCompatibleBitmap(hdc: isize, w: i32, h: i32) -> isize;
    fn SelectObject(hdc: isize, obj: isize) -> isize;
    fn DeleteObject(obj: isize) -> i32;
    fn DeleteDC(hdc: isize) -> i32;
    fn GetDIBits(
        hdc: isize,
        bmp: isize,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
}

const PW_RENDERFULLCONTENT: u32 = 2;

struct Found {
    hwnd: isize,
}

extern "system" fn probe(hwnd: isize, lp: isize) -> i32 {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != (lp as u32) || IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        // NSH 会先建一个约 199×34 的辅助顶层窗口：按“可见 + 够大 + 标题带 envboard”挑。
        let mut rect = Rect { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return 1;
        }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        // 主窗是 1618×1060；NSH 会先建 ~199×34 的辅助窗，还有可能缩过的小窗，
        // 一律按“大于 1000×700”筛，宁可等也不抓错。
        if w < 1000 || h < 700 {
            return 1;
        }
        let mut title = [0u16; 128];
        let n = GetWindowTextW(hwnd, title.as_mut_ptr(), 128) as usize;
        let title = String::from_utf16_lossy(&title[..n.min(128)]);
        if !title.contains("envboard") {
            return 1;
        }
        // 把句柄写回调用方栈上的槽位（lp 指向它可以，简单起见用全局）。
        WINDOW.store(hwnd, std::sync::atomic::Ordering::SeqCst);
        1
    }
}

static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn wait_for_window(pid: u32, timeout: Duration) -> Found {
    let started = Instant::now();
    while started.elapsed() < timeout {
        WINDOW.store(0, std::sync::atomic::Ordering::SeqCst);
        unsafe {
            EnumWindows(probe, pid as isize);
        }
        let hwnd = WINDOW.load(std::sync::atomic::Ordering::SeqCst);
        if hwnd != 0 {
            return Found { hwnd };
        }
        sleep(Duration::from_millis(150));
    }
    panic!("等了 {timeout:?} 也没等到 envboard 主窗口");
}

/// 整窗 BGRA（自上而下）。只走 `PrintWindow`：两条路径（PrintWindow / 桌面 BitBlt）
/// 会在圆角与边框上给出不同的像素 —— 一次抓图只能有一条路径，否则基线自己就不稳。
fn capture(found: &Found) -> (u32, u32, Vec<u8>) {
    unsafe {
        let mut rect = Rect { left: 0, top: 0, right: 0, bottom: 0 };
        assert!(GetWindowRect(found.hwnd, &mut rect) != 0, "GetWindowRect 失败");
        let w = (rect.right - rect.left).max(1);
        let h = (rect.bottom - rect.top).max(1);

        let wdc = GetWindowDC(found.hwnd);
        let mem = CreateCompatibleDC(wdc);
        let bmp = CreateCompatibleBitmap(wdc, w, h);
        let old = SelectObject(mem, bmp);

        assert!(
            PrintWindow(found.hwnd, mem, PW_RENDERFULLCONTENT) != 0,
            "PrintWindow 失败（窗口可能还没画出首帧）"
        );
        let pixels = read_back(mem, bmp, w, h);

        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(found.hwnd, wdc);

        let colours = distinct_colours(&pixels);
        assert!(
            colours > 8,
            "抓到的画面只有 {colours} 种颜色，基本是空白：PrintWindow 没拿到 D2D 内容"
        );
        (w as u32, h as u32, pixels)
    }
}

unsafe fn read_back(mem: isize, bmp: isize, w: i32, h: i32) -> Vec<u8> {
    let mut info = BitmapInfo {
        header: BitmapInfoHeader {
            size: std::mem::size_of::<BitmapInfoHeader>() as u32,
            width: w,
            // 负高度 = 自上而下，省掉一次翻转。
            height: -h,
            planes: 1,
            bit_count: 32,
            compression: 0, // BI_RGB
            size_image: 0,
            x_pels_per_meter: 0,
            y_pels_per_meter: 0,
            clr_used: 0,
            clr_important: 0,
        },
        colors: [0; 3],
    };
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let lines = unsafe {
        GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            pixels.as_mut_ptr().cast::<c_void>(),
            &mut info,
            0, // DIB_RGB_COLORS
        )
    };
    assert_eq!(lines, h, "GetDIBits 只拿到 {lines} 行");
    pixels
}

fn distinct_colours(pixels: &[u8]) -> usize {
    let mut seen = std::collections::BTreeSet::new();
    for px in pixels.chunks_exact(4).step_by(97) {
        seen.insert((px[0], px[1], px[2], px[3]));
        if seen.len() > 64 {
            break;
        }
    }
    seen.len()
}

// ---- 基线 -----------------------------------------------------------------

/// GDI 的 32bpp DIB 是 BGRA，PNG 要 RGBA：只换第 1、3 字节。
fn to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgra.len());
    for px in bgra.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], 0xFF]);
    }
    out
}

fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    let file = fs::File::create(path).expect("建基线文件失败");
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("写 PNG 头失败");
    writer.write_image_data(rgba).expect("写 PNG 像素失败");
}

fn load_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = fs::File::open(path).expect("读基线失败");
    let mut reader = png::Decoder::new(file).read_info().expect("PNG 头损坏");
    assert_eq!(
        reader.info().color_type,
        png::ColorType::Rgba,
        "基线必须是 RGBA8：{}",
        path.display()
    );
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("PNG 像素损坏");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// 差异数量 + 外接框（`x0,y0,x1,y1`）。框比总数更值钱：它一句话说清“哪块变了”。
fn diff_report(expected: &[u8], actual: &[u8], w: u32) -> (usize, Option<(u32, u32, u32, u32)>) {
    let mut count = 0;
    let mut bbox: Option<(u32, u32, u32, u32)> = None;
    for (i, (a, b)) in expected.chunks_exact(4).zip(actual.chunks_exact(4)).enumerate() {
        if a == b {
            continue;
        }
        count += 1;
        let x = i as u32 % w;
        let y = i as u32 / w;
        bbox = Some(match bbox {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    (count, bbox)
}

// ---- 测试 -----------------------------------------------------------------

#[test]
#[ignore = "需要真桌面：会拉起 exe、等窗口、抓整窗像素；换 DPI 要 UPDATE_GOLDEN=1 重拍"]
fn golden_pages_match_baselines() {
    // DPI 必须钉死且**与绘制一致**：Per-Monitor v2 下 `GetWindowRect` 与
    // `PrintWindow` 都按物理像素走，1920/2560 宽屏 125% 就是 1618×1060。
    // 若设成 unaware，测量得到虚拟化尺寸（1294×848）而画面仍按物理尺寸画，
    // 会右下裁掉一条 —— 这个坑踩过一次，别再踩。
    unsafe {
        SetProcessDpiAwarenessContext(-4); // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
    }

    let update = std::env::var("UPDATE_GOLDEN").map(|v| v == "1").unwrap_or(false);
    let dir = golden_dir();
    fs::create_dir_all(&dir).expect("建 tests/golden 失败");

    let mut failures = Vec::new();
    for (name, envs) in CASES {
        let child = launch(envs);
        let found = wait_for_window(child.0.id(), Duration::from_secs(15));
        // 鼠标若停在窗口里，悬停色会进基线（`hover_fill` 一行 66k 像素的量级）。
        // 抓图前把光标赶出窗口：窗口在 0,0、1618×1060，屏幕右下角是安全的。
        unsafe {
            SetCursorPos(GetSystemMetrics(0) - 4, GetSystemMetrics(1) - 4);
        }
        sleep(Duration::from_millis(600)); // 首帧绘制完 + 悬停态淡出
        let (w, h, bgra) = capture(&found);
        let rgba = to_rgba(&bgra);

        let path = dir.join(format!("{name}.png"));
        if update || !path.exists() {
            save_png(&path, w, h, &rgba);
            eprintln!("golden: 写入基线 {name}（{w}×{h}）");
            continue;
        }

        let (bw, bh, expected) = load_png(&path);
        if (w, h) != (bw, bh) {
            failures.push(format!(
                "{name}: 尺寸 {w}×{h} 与基线 {bw}×{bh} 不符 —— 换了 DPI 或改过窗口尺寸？\
                 确认后 UPDATE_GOLDEN=1 重拍"
            ));
            continue;
        }
        let (pixels_diff, bbox) = diff_report(&expected, &rgba, w);
        if pixels_diff != 0 {
            let actual = dir.join(format!("{name}.actual.png"));
            save_png(&actual, w, h, &rgba);
            let where_ = bbox
                .map(|(x0, y0, x1, y1)| format!("，差异框 x{x0}..{x1} y{y0}..{y1}"))
                .unwrap_or_default();
            failures.push(format!(
                "{name}: {pixels_diff} 个像素不同（基线 {bw}×{bh}{where_}）；实际图在 {}",
                actual.display()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "golden 不一致（{}\n提示：改原语不该动页面；页面确实要变时用 UPDATE_GOLDEN=1 重拍并肉眼过一遍）",
        failures.join("\n")
    );
}
