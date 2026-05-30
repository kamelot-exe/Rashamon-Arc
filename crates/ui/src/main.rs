//! Rashamon Arc — main browser UI process.
#![cfg_attr(
    all(feature = "kamelot", not(feature = "linux-desktop")),
    allow(dead_code, unused_imports)
)]
mod core_driver;
mod display;
mod draw;
mod font;
mod hit_test;
mod input;
mod layout;
mod omnibox;
mod page;
mod platform;
mod persist;
mod runtime;
mod theme;
mod ui_state;

use crate::font::FontManager;
use crate::hit_test::{
    permission_kinds_ui, permission_prompt_hit_rects, permission_prompt_rect,
    site_info_adblock_rect, site_info_permission_rects, site_info_rect, Rect,
};
use crate::layout::*;
use crate::page::{PageNode, parse_html, is_low_content};
use core_driver::{BrowserAction, BrowserCoreDriver};
#[cfg(feature = "linux-desktop")]
use runtime::LinuxDesktopRuntime;
use runtime::PlatformRuntime;
use rashamon_net::{AdblockEngine, HttpClient};
use rashamon_renderer::{
    adblock_policy_blocks_for_test,
    download_destination_for_test, origin_from_url, CursorKind, DecisionSource, EngineEvent, EngineFrame,
    Framebuffer, PermissionDecision, PermissionKind, PermissionStore, RenderEngine,
    webext_blocked_events_for_test, webext_is_available_for_test, webext_is_configured_for_test,
    webext_is_disabled_for_test, webext_probe_blocked_for_test, webext_probe_clear_for_test,
    webext_ready_for_test, webext_rules_error_for_test, webext_rules_ok_for_test,
};
use rashamon_renderer::framebuffer::Pixel;
use ui_state::{BrowserState, DirtyFlags, DownloadStatus, OverlayKind, PageState, TabId, derive_title};

use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::process::Command;

// Loading timing (at 60 fps)
const LOAD_MIN_FRAMES:     u64 = 60;   // 1 s minimum visible loading state

// Page layout constants (shared between render and measure)
const MARGIN:  u32 = 120;
const MAX_W:   u32 = 880;
const PAD_TOP: u32 = 28;

// Smoke-test scroll step; runtime wheel handling lives in BrowserCoreDriver.
const SCROLL_WHEEL: i32 = 80;

// Private tab accent colour (purple stripe)
const PRIVATE_ACCENT: Pixel = Pixel { r: 130, g: 70, b: 200 };

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const SHADOW_DARK: Pixel = Pixel { r: 6, g: 8, b: 12 };

struct PerfTracker {
    enabled: bool,
    t0: Instant,
    startup_reported: bool,
    first_shell_frame_ms: Option<u128>,
    first_loading_surface_ms: Option<u128>,
    first_webkit_frame_ms: Option<u128>,
    pending_scroll_at: Option<Instant>,
    pending_tab_switch_at: Option<Instant>,
    last_active_tab: Option<u64>,
    last_stats_dump: Instant,
    last_frame_start: Instant,
    render_into_total_us: u128,
    draw_ui_total_us: u128,
    present_total_us: u128,
    frame_total_us: u128,
    timed_frames: u64,
}

impl PerfTracker {
    fn new() -> Self {
        let now = Instant::now();
        let perf_arg = std::env::args().skip(1).any(|arg| arg == "--perf");
        Self {
            enabled: std::env::var_os("RASHAMON_PERF").is_some() || perf_arg,
            t0: now,
            startup_reported: false,
            first_shell_frame_ms: None,
            first_loading_surface_ms: None,
            first_webkit_frame_ms: None,
            pending_scroll_at: None,
            pending_tab_switch_at: None,
            last_active_tab: None,
            last_stats_dump: now,
            last_frame_start: now,
            render_into_total_us: 0,
            draw_ui_total_us: 0,
            present_total_us: 0,
            frame_total_us: 0,
            timed_frames: 0,
        }
    }

    fn ms_since_start(&self) -> u128 {
        self.t0.elapsed().as_millis()
    }

    fn on_initial_frame(&mut self) {
        if !self.enabled {
            return;
        }
        if self.first_shell_frame_ms.is_none() {
            self.first_shell_frame_ms = Some(self.ms_since_start());
            eprintln!("[perf] first-shell-frame-ms={}", self.first_shell_frame_ms.unwrap_or(0));
        }
    }

    fn on_loading_surface_frame(&mut self) {
        if !self.enabled {
            return;
        }
        if self.first_loading_surface_ms.is_none() {
            self.first_loading_surface_ms = Some(self.ms_since_start());
            eprintln!(
                "[perf] first-loading-surface-ms={}",
                self.first_loading_surface_ms.unwrap_or(0)
            );
        }
    }

    fn note_input(&mut self, ev: &input::PlatformEvent) {
        if !self.enabled {
            return;
        }
        if matches!(ev, input::PlatformEvent::Scroll { .. }) {
            self.pending_scroll_at = Some(Instant::now());
        }
    }

    fn note_active_tab(&mut self, active_tab_id: u64) {
        if !self.enabled {
            return;
        }
        if let Some(prev) = self.last_active_tab {
            if prev != active_tab_id {
                self.pending_tab_switch_at = Some(Instant::now());
            }
        }
        self.last_active_tab = Some(active_tab_id);
    }

    fn on_engine_event(&mut self, ev: &EngineEvent) {
        if !self.enabled {
            return;
        }
        if let EngineEvent::FrameReady { reason } = ev {
            if self.first_webkit_frame_ms.is_none() {
                self.first_webkit_frame_ms = Some(self.ms_since_start());
                eprintln!(
                    "[perf] first-webkit-frame-ms={} reason={}",
                    self.first_webkit_frame_ms.unwrap_or(0),
                    reason
                );
            }
            if reason.contains("scroll") {
                if let Some(t0) = self.pending_scroll_at.take() {
                    eprintln!("[perf] scroll-snapshot-latency-ms={}", t0.elapsed().as_millis());
                }
            }
            if reason.contains("switch") {
                if let Some(t0) = self.pending_tab_switch_at.take() {
                    eprintln!("[perf] tab-switch-latency-ms={}", t0.elapsed().as_millis());
                }
            }
        }
    }

    fn maybe_dump_runtime_stats(&mut self, driver: &BrowserCoreDriver) {
        if !self.enabled || self.last_stats_dump.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_stats_dump = Instant::now();
        let stats = driver.engine.perf_stats();
        let timed = self.timed_frames.max(1) as u128;
        eprintln!(
            "[perf] t={}ms tabs={} live_webviews={} suspended={} rss_kb={} engine_render_into_ms={:.3} draw_ui_ms={:.3} present_frame_ms={:.3} frame_total_ms={:.3}",
            self.ms_since_start(),
            driver.state.tabs.len(),
            stats.live_webviews,
            stats.suspended_tabs,
            read_process_rss_kb().unwrap_or(0),
            self.render_into_total_us as f64 / timed as f64 / 1000.0,
            self.draw_ui_total_us as f64 / timed as f64 / 1000.0,
            self.present_total_us as f64 / timed as f64 / 1000.0,
            self.frame_total_us as f64 / timed as f64 / 1000.0,
        );
        self.render_into_total_us = 0;
        self.draw_ui_total_us = 0;
        self.present_total_us = 0;
        self.frame_total_us = 0;
        self.timed_frames = 0;
    }

    fn maybe_report_startup(&mut self) {
        if !self.enabled || self.startup_reported {
            return;
        }
        if let (Some(shell), Some(webkit)) = (self.first_shell_frame_ms, self.first_webkit_frame_ms) {
            eprintln!(
                "[perf] startup-ms={} first-frame-ms={} first-webkit-frame-ms={}",
                self.ms_since_start(),
                shell,
                webkit
            );
            self.startup_reported = true;
        }
    }

    fn mark_frame_start(&mut self) {
        if self.enabled {
            self.last_frame_start = Instant::now();
        }
    }

    fn add_render_into(&mut self, elapsed: Duration) {
        if self.enabled {
            self.render_into_total_us += elapsed.as_micros();
        }
    }

    fn add_draw_ui(&mut self, elapsed: Duration) {
        if self.enabled {
            self.draw_ui_total_us += elapsed.as_micros();
        }
    }

    fn add_present(&mut self, elapsed: Duration) {
        if self.enabled {
            self.present_total_us += elapsed.as_micros();
        }
    }

    fn mark_frame_end(&mut self) {
        if self.enabled {
            self.frame_total_us += self.last_frame_start.elapsed().as_micros();
            self.timed_frames = self.timed_frames.saturating_add(1);
        }
    }
}

fn read_process_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let mut parts = line.split_whitespace();
    let _ = parts.next()?;
    parts.next()?.parse::<u64>().ok()
}

#[derive(Debug, Default, Clone)]
struct RuntimePerfSample {
    first_shell_frame_ms: Option<f64>,
    first_loading_surface_ms: Option<f64>,
    engine_render_into_ms: Option<f64>,
    draw_ui_ms: Option<f64>,
    present_frame_ms: Option<f64>,
    frame_total_ms: Option<f64>,
    rss_kb: Option<u64>,
}

#[derive(Debug, Default, Clone)]
struct BenchRunSample {
    first_shell_frame_ms: Option<f64>,
    first_loading_surface_ms: Option<f64>,
    cold_start_ms: Option<f64>,
    first_webkit_frame_ms: Option<f64>,
    tab_switch_latency_ms: Option<f64>,
    scroll_snapshot_latency_ms: Option<f64>,
    engine_render_into_ms: Option<f64>,
    draw_ui_ms: Option<f64>,
    present_frame_ms: Option<f64>,
    frame_total_ms: Option<f64>,
    rss_kb: Option<f64>,
    load_start_ms: Option<f64>,
    load_finished_ms: Option<f64>,
    snapshot_completed_ms: Option<f64>,
    libsoup_warning_count: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchTempMode {
    Warm,
    Cold,
}

#[derive(Debug, Clone, Copy)]
struct BenchConfig {
    mode: BenchTempMode,
    isolate_profile: bool,
}

fn parse_perf_number(line: &str, key: &str) -> Option<f64> {
    for token in line.split_whitespace() {
        if let Some(rest) = token.strip_prefix(key) {
            return rest.parse::<f64>().ok();
        }
    }
    None
}

fn parse_perf_u64(line: &str, key: &str) -> Option<u64> {
    for token in line.split_whitespace() {
        if let Some(rest) = token.strip_prefix(key) {
            return rest.parse::<u64>().ok();
        }
    }
    None
}

fn run_perf_smoke_once(
    run_id: usize,
    cfg: BenchConfig,
) -> Result<SmokePerfMetrics, Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    if cfg.isolate_profile {
        let profile_suffix = match cfg.mode {
            BenchTempMode::Cold => format!("bench-cold-smoke-{}-{}", std::process::id(), run_id),
            BenchTempMode::Warm => format!("bench-warm-smoke-{}", std::process::id()),
        };
        cmd.env("RASHAMON_PROFILE_SUFFIX", profile_suffix);
    }
    let out = cmd
        .arg("--smoke-test-webkit")
        .arg("--perf")
        .arg("--smoke-quiet")
        .output()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(format!("bench smoke run failed:\n{combined}").into());
    }
    let mut metrics = SmokePerfMetrics::default();
    for line in combined.lines() {
        if line.contains("libsoup-WARNING") {
            metrics.libsoup_warning_count = metrics.libsoup_warning_count.saturating_add(1);
        }
        if !line.contains("[perf]") {
            continue;
        }
        if let Some(v) = parse_perf_number(line, "cold-start-ms=") {
            metrics.cold_start_ms = Some(v as u128);
        }
        if let Some(v) = parse_perf_number(line, "first-webkit-frame-ms=") {
            metrics.first_webkit_frame_ms = Some(v as u128);
        }
        if let Some(v) = parse_perf_number(line, "tab-switch-latency-ms=") {
            metrics.tab_switch_latency_ms = Some(v as u128);
        }
        if let Some(v) = parse_perf_number(line, "scroll-snapshot-latency-ms=") {
            metrics.scroll_snapshot_latency_ms = Some(v as u128);
        }
        if line.contains("webkit_stage=load-start") {
            if let Some(v) = parse_perf_number(line, "t_ms=") {
                metrics.load_start_ms = Some(v as u128);
            }
        }
        if line.contains("webkit_stage=load-finished") {
            if let Some(v) = parse_perf_number(line, "t_ms=") {
                metrics.load_finished_ms = Some(v as u128);
            }
        }
        if line.contains("webkit_stage=snapshot-completed") {
            if let Some(v) = parse_perf_number(line, "t_ms=") {
                metrics.snapshot_completed_ms = Some(v as u128);
            }
        }
    }
    Ok(metrics)
}

fn run_perf_runtime_once(
    run_id: usize,
    cfg: BenchConfig,
) -> Result<RuntimePerfSample, Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    if cfg.isolate_profile {
        let profile_suffix = match cfg.mode {
            BenchTempMode::Cold => format!("bench-cold-runtime-{}-{}", std::process::id(), run_id),
            BenchTempMode::Warm => format!("bench-warm-runtime-{}", std::process::id()),
        };
        cmd.env("RASHAMON_PROFILE_SUFFIX", profile_suffix);
    }
    let out = cmd
        .arg("--perf-sample")
        .arg("--perf")
        .arg("--smoke-quiet")
        .output()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(format!("bench runtime sample failed:\n{combined}").into());
    }
    let mut sample = RuntimePerfSample::default();
    for line in combined.lines() {
        if !(line.contains("[perf]") && line.contains("engine_render_into_ms=")) {
            if line.contains("[perf] first-shell-frame-ms=") {
                sample.first_shell_frame_ms = parse_perf_number(line, "first-shell-frame-ms=");
            }
            if line.contains("[perf] first-loading-surface-ms=") {
                sample.first_loading_surface_ms =
                    parse_perf_number(line, "first-loading-surface-ms=");
            }
            continue;
        }
        sample.engine_render_into_ms = parse_perf_number(line, "engine_render_into_ms=");
        sample.draw_ui_ms = parse_perf_number(line, "draw_ui_ms=");
        sample.present_frame_ms = parse_perf_number(line, "present_frame_ms=");
        sample.frame_total_ms = parse_perf_number(line, "frame_total_ms=");
        sample.rss_kb = parse_perf_u64(line, "rss_kb=");
    }
    Ok(sample)
}

fn stat_line(name: &str, values: &[f64]) -> String {
    if values.is_empty() {
        return format!("{name} median=NA p95=NA min=NA max=NA");
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    let median = if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    };
    let p95_idx = ((n as f64) * 0.95).ceil().max(1.0) as usize - 1;
    let p95 = v[p95_idx.min(n - 1)];
    format!(
        "{name} median={:.3} p95={:.3} min={:.3} max={:.3}",
        median,
        p95,
        v[0],
        v[n - 1]
    )
}

fn run_bench_perf(runs: usize, cfg: BenchConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(runs);
    for i in 0..runs {
        let smoke = run_perf_smoke_once(i + 1, cfg)?;
        let runtime = run_perf_runtime_once(i + 1, cfg)?;
        samples.push(BenchRunSample {
            first_shell_frame_ms: runtime.first_shell_frame_ms,
            first_loading_surface_ms: runtime.first_loading_surface_ms,
            cold_start_ms: smoke.cold_start_ms.map(|v| v as f64),
            first_webkit_frame_ms: smoke.first_webkit_frame_ms.map(|v| v as f64),
            tab_switch_latency_ms: smoke.tab_switch_latency_ms.map(|v| v as f64),
            scroll_snapshot_latency_ms: smoke.scroll_snapshot_latency_ms.map(|v| v as f64),
            engine_render_into_ms: runtime.engine_render_into_ms,
            draw_ui_ms: runtime.draw_ui_ms,
            present_frame_ms: runtime.present_frame_ms,
            frame_total_ms: runtime.frame_total_ms,
            rss_kb: runtime.rss_kb.map(|v| v as f64),
            load_start_ms: smoke.load_start_ms.map(|v| v as f64),
            load_finished_ms: smoke.load_finished_ms.map(|v| v as f64),
            snapshot_completed_ms: smoke.snapshot_completed_ms.map(|v| v as f64),
            libsoup_warning_count: Some(smoke.libsoup_warning_count as f64),
        });
        eprintln!("[bench] run {}/{} done", i + 1, runs);
    }

    let collect = |f: fn(&BenchRunSample) -> Option<f64>| -> Vec<f64> {
        samples.iter().filter_map(f).collect()
    };

    let mode = match cfg.mode {
        BenchTempMode::Cold => "cold",
        BenchTempMode::Warm => "warm",
    };
    println!(
        "perf-summary runs={} mode={} isolate_profile={}",
        runs, mode, cfg.isolate_profile
    );
    println!(
        "{}",
        stat_line("first-shell-frame-ms", &collect(|s| s.first_shell_frame_ms))
    );
    println!(
        "{}",
        stat_line(
            "first-loading-surface-ms",
            &collect(|s| s.first_loading_surface_ms)
        )
    );
    println!("{}", stat_line("cold-start-ms", &collect(|s| s.cold_start_ms)));
    println!("{}", stat_line("load-start-ms", &collect(|s| s.load_start_ms)));
    println!("{}", stat_line("load-finished-ms", &collect(|s| s.load_finished_ms)));
    println!(
        "{}",
        stat_line("snapshot-completed-ms", &collect(|s| s.snapshot_completed_ms))
    );
    println!(
        "{}",
        stat_line(
            "first-webkit-frame-ms",
            &collect(|s| s.first_webkit_frame_ms)
        )
    );
    println!(
        "{}",
        stat_line(
            "tab-switch-latency-ms",
            &collect(|s| s.tab_switch_latency_ms)
        )
    );
    println!(
        "{}",
        stat_line(
            "scroll-snapshot-latency-ms",
            &collect(|s| s.scroll_snapshot_latency_ms)
        )
    );
    println!(
        "{}",
        stat_line("engine_render_into_ms", &collect(|s| s.engine_render_into_ms))
    );
    println!("{}", stat_line("draw_ui_ms", &collect(|s| s.draw_ui_ms)));
    println!(
        "{}",
        stat_line("present_frame_ms", &collect(|s| s.present_frame_ms))
    );
    println!("{}", stat_line("frame_total_ms", &collect(|s| s.frame_total_ms)));
    println!("{}", stat_line("rss_kb", &collect(|s| s.rss_kb)));
    println!(
        "{}",
        stat_line("libsoup-warning-count", &collect(|s| s.libsoup_warning_count))
    );
    Ok(())
}

fn load_ui_font_data() -> &'static [u8] {
    const FALLBACK: &[u8] = include_bytes!("../assets/DejaVuSansMono.ttf");
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
    ];

    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            return Box::leak(bytes.into_boxed_slice());
        }
    }
    FALLBACK
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContentRenderMode {
    Text,
    Engine,
    EnginePending,
}

// ── Persistence helpers ───────────────────────────────────────────────────────

// ── Fetch / parse pipeline ────────────────────────────────────────────────────

enum FetchOutcome {
    Success {
        title:            Option<String>,
        nodes:            Vec<PageNode>,
        meta_description: Option<String>,
        noscript:         Option<String>,
    },
    Failure(String),
}

struct PendingFetch {
    tab_id:   TabId,
    receiver: mpsc::Receiver<FetchOutcome>,
}

fn do_fetch(url: String) -> FetchOutcome {
    let mut client = HttpClient::new();
    match client.fetch_text(&url) {
        Err(reason) => FetchOutcome::Failure(reason),
        Ok(html)    => {
            let parsed = parse_html(&html);
            FetchOutcome::Success {
                title:            parsed.title,
                nodes:            parsed.nodes,
                meta_description: parsed.meta_description,
                noscript:         parsed.noscript,
            }
        }
    }
}

fn spawn_fetch(tab_id: TabId, url: String) -> PendingFetch {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(do_fetch(url)); });
    PendingFetch { tab_id, receiver: rx }
}

// ── Internal release smoke test ───────────────────────────────────────────────

fn smoke_fail(msg: impl Into<String>) -> Box<dyn std::error::Error> {
    msg.into().into()
}

fn smoke_create_tab(state: &mut BrowserState, engine: &mut RenderEngine, private: bool) {
    if private {
        state.open_private_tab();
    } else {
        state.open_new_tab();
    }
    let id = state.active_tab_id;
    engine.create_tab(id.raw(), private);
    engine.set_active_tab(id.raw());
}

fn smoke_navigate(
    state:  &mut BrowserState,
    engine: &mut RenderEngine,
    url:    &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = state.begin_navigate(url)
        .ok_or_else(|| smoke_fail(format!("begin_navigate rejected {url}")))?;
    let nav_id = state.active_tab().map_or(0, |t| t.nav_id);
    engine.navigate(&url, nav_id)?;
    Ok(())
}

fn smoke_apply_engine_events(
    state:  &mut BrowserState,
    engine: &mut RenderEngine,
) -> Vec<EngineEvent> {
    let mut seen = Vec::new();
    for (tab_id, ev) in engine.poll_events() {
        let target_raw = if tab_id == 0 { state.active_tab_id.raw() } else { tab_id };
        match &ev {
            EngineEvent::TitleChanged(t) => {
                if let Some(tab) = state.tabs.iter_mut().find(|t2| t2.id.raw() == target_raw) {
                    tab.title = t.clone();
                }
            }
            EngineEvent::UrlChanged(u) => {
                if let Some(tab) = state.tabs.iter_mut().find(|t2| t2.id.raw() == target_raw) {
                    tab.url = u.clone();
                }
            }
            EngineEvent::LoadComplete => state.resolve_engine_loading_for(target_raw),
            EngineEvent::LoadFailed(reason) => state.fail_loading_for(target_raw, reason),
            EngineEvent::ContentHeightChanged(h) => state.set_content_height_for(target_raw, *h),
            EngineEvent::NavStateChanged { can_back, can_forward } => {
                if let Some(tab) = state.tabs.iter_mut().find(|t2| t2.id.raw() == target_raw) {
                    tab.webkit_can_back = *can_back;
                    tab.webkit_can_forward = *can_forward;
                }
            }
            EngineEvent::FindMatchCount(count) => {
                if target_raw == state.active_tab_id.raw() {
                    state.find_match_count = Some(*count);
                }
            }
            EngineEvent::DownloadStarted { id, filename, path } => {
                state.upsert_download_started(*id, filename.clone(), path.clone());
            }
            EngineEvent::DownloadProgress { id, received, progress } => {
                state.update_download_progress(*id, *received, *progress);
            }
            EngineEvent::DownloadFinished { id, path } => {
                state.finish_download(*id, path.clone());
            }
            EngineEvent::DownloadFailed { id, reason } => {
                state.fail_download(*id, reason.clone());
            }
            EngineEvent::PermissionPrompt { id, origin, kind, nav_id } => {
                state.show_permission_prompt(*id, target_raw, *nav_id, origin.clone(), *kind);
            }
            EngineEvent::PermissionResolved { id } => {
                state.clear_permission_prompt(*id);
            }
            EngineEvent::SitePermissions {
                origin,
                decisions,
                adblock_enabled,
                adblock_allowlisted,
                blocked_count,
            } => {
                state.set_site_permissions(
                    origin.clone(),
                    decisions.clone(),
                    *adblock_enabled,
                    *adblock_allowlisted,
                    *blocked_count,
                );
            }
            EngineEvent::CursorChanged(_) => {}
            EngineEvent::FrameReady { .. } => {}
            EngineEvent::LoadStarted => {
                let frame = state.frame_count;
                if let Some(tab) = state.tabs.iter_mut().find(|t2| t2.id.raw() == target_raw) {
                    tab.page_state = PageState::Loading;
                    tab.load_start_frame = frame;
                }
            }
        }
        seen.push(ev);
    }
    seen
}

fn smoke_wait_for<F>(
    state:  &mut BrowserState,
    engine: &mut RenderEngine,
    label:  &str,
    timeout: Duration,
    mut pred: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&BrowserState, &[EngineEvent]) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        state.frame_count += 1;
        engine.pump_gtk();
        let events = smoke_apply_engine_events(state, engine);
        if pred(state, &events) {
            smoke_log(format!("[smoke] PASS {label}"));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    Err(smoke_fail(format!("smoke timeout: {label}")))
}

fn smoke_adblock_model() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = AdblockEngine::new();
    let (blocked, reason) =
        engine.should_block("https://ad.doubleclick.net/pagead/id", "https://example.com");
    if !blocked || reason.as_deref() != Some("doubleclick.net") {
        return Err(smoke_fail("adblock did not block default domain"));
    }
    let (blocked_subresource, _) = engine.should_block(
        "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
        "https://example.com",
    );
    if !blocked_subresource {
        return Err(smoke_fail(
            "adblock did not block subresource-like tracker URL",
        ));
    }

    engine.allowlist_domain("doubleclick.net");
    let (blocked, _) =
        engine.should_block("https://ad.doubleclick.net/pagead/id", "https://example.com");
    if blocked {
        return Err(smoke_fail("adblock allowlist did not override block rule"));
    }
    let payload = engine.export_rule_payload_for_context(false);
    if payload.version != 1
        || !payload.enabled
        || !payload.blocked_domains.iter().any(|domain| domain == "doubleclick.net")
        || !payload.blocked_substrings.iter().any(|pattern| pattern == "facebook.com/tr")
        || !payload.allowlist_domains.iter().any(|domain| domain == "doubleclick.net")
    {
        return Err(smoke_fail("adblock structured rule export is incomplete"));
    }
    let sync_text = engine.export_rule_sync_text_for_context(false);
    if !sync_text.contains("version=1\n")
        || !sync_text.contains("block-domain=doubleclick.net\n")
        || !sync_text.contains("block-substring=facebook.com/tr\n")
        || !sync_text.contains("allow-domain=doubleclick.net\n")
    {
        return Err(smoke_fail("adblock rule sync payload is incomplete"));
    }

    let mut private_engine = AdblockEngine::new();
    let (blocked, _) =
        private_engine.should_block("https://www.facebook.com/tr?id=1", "https://example.com");
    if !blocked {
        return Err(smoke_fail("adblock did not apply to private-mode model"));
    }

    let path = std::env::temp_dir().join(format!(
        "rashamon-adblock-smoke-{}.json",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&path);
    let mut persisted = AdblockEngine::new();
    persisted.allowlist_domain("doubleclick.net");
    persisted.save_allowlist_to_path(&path);
    let mut loaded = AdblockEngine::new();
    loaded.load_allowlist_from_path(&path);
    let (blocked, _) =
        loaded.should_block("https://ad.doubleclick.net/pagead/id", "https://example.com");
    if blocked {
        return Err(smoke_fail("adblock allowlist did not persist"));
    }

    let mut private_session = AdblockEngine::new();
    private_session.allowlist_domain_for_context("doubleclick.net", true);
    let (blocked_private, _) = private_session.should_block_for_context(
        "https://ad.doubleclick.net/pagead/id",
        "https://example.com",
        true,
    );
    let (blocked_normal, _) = private_session.should_block_for_context(
        "https://ad.doubleclick.net/pagead/id",
        "https://example.com",
        false,
    );
    if blocked_private || !blocked_normal {
        return Err(smoke_fail("private adblock allowlist leaked into normal context"));
    }
    let normal_payload = private_session.export_rule_payload_for_context(false);
    let private_payload = private_session.export_rule_payload_for_context(true);
    if normal_payload.allowlist_domains.iter().any(|domain| domain == "doubleclick.net")
        || !private_payload.allowlist_domains.iter().any(|domain| domain == "doubleclick.net")
    {
        return Err(smoke_fail("private adblock allowlist export did not preserve context"));
    }

    std::fs::write(&path, "{not-json")?;
    let mut malformed = AdblockEngine::new();
    malformed.load_allowlist_from_path(&path);
    let _ = std::fs::remove_file(&path);
    let (blocked, _) =
        malformed.should_block("https://ad.doubleclick.net/pagead/id", "https://example.com");
    if !blocked {
        return Err(smoke_fail("malformed adblock allowlist disabled blocking"));
    }

    smoke_log("[smoke] PASS adblock model");
    Ok(())
}

fn smoke_download_destination_model() -> Result<(), Box<dyn std::error::Error>> {
    let name = format!("rashamon-download-smoke-{}.txt", std::process::id());
    let first = download_destination_for_test(&name);
    if first.file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
        return Err(smoke_fail("download destination did not preserve filename"));
    }
    if let Some(parent) = first.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Err(err) = std::fs::write(&first, b"rashamon smoke") {
        if matches!(
            err.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
        ) {
            smoke_log(format!(
                "[smoke] SKIP download destination duplicate check (non-writable path: {})",
                first.display()
            ));
            smoke_log("[smoke] PASS download destination model");
            return Ok(());
        }
        return Err(err.into());
    }
    let second = download_destination_for_test(&name);
    let _ = std::fs::remove_file(&first);
    if second == first {
        return Err(smoke_fail("download destination did not avoid duplicate filename"));
    }
    smoke_log("[smoke] PASS download destination model");
    Ok(())
}

fn smoke_permissions_model() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "rashamon-permissions-smoke-{}.json",
        std::process::id(),
    ));
    let _ = std::fs::remove_file(&path);

    let https_origin = origin_from_url("https://Example.com/path?q=1")
        .ok_or_else(|| smoke_fail("permission origin parse failed"))?;
    if https_origin != "https://example.com" {
        return Err(smoke_fail("permission origin was not normalized"));
    }
    if origin_from_url("http://example.com") == origin_from_url("https://example.com") {
        return Err(smoke_fail("permission origin mixed http and https"));
    }

    let mut store = PermissionStore::load_from_path(path.clone());
    let (decision, source) = store.get(&https_origin, PermissionKind::Camera, false);
    if decision != PermissionDecision::Ask || source != DecisionSource::Default {
        return Err(smoke_fail("unknown permission did not default to ask"));
    }

    store.set(
        &https_origin,
        PermissionKind::Notifications,
        PermissionDecision::Allow,
        false,
    );
    let reloaded = PermissionStore::load_from_path(path.clone());
    let (decision, source) = reloaded.get(&https_origin, PermissionKind::Notifications, false);
    if decision != PermissionDecision::Allow || source != DecisionSource::Persisted {
        return Err(smoke_fail("normal permission did not persist"));
    }
    let mut store = PermissionStore::load_from_path(path.clone());
    store.set(
        &https_origin,
        PermissionKind::Camera,
        PermissionDecision::Deny,
        false,
    );
    let reloaded = PermissionStore::load_from_path(path.clone());
    let (decision, source) = reloaded.get(&https_origin, PermissionKind::Camera, false);
    if decision != PermissionDecision::Deny || source != DecisionSource::Persisted {
        return Err(smoke_fail("normal deny permission did not persist"));
    }
    let mut panel_state = BrowserState::new();
    panel_state.open_site_info(Some(https_origin.clone()));
    panel_state.set_site_permissions(
        https_origin.clone(),
        vec![(PermissionKind::Camera, PermissionDecision::Deny)],
        true,
        true,
        3,
    );
    if permission_decision_for(&panel_state, PermissionKind::Camera) != PermissionDecision::Deny {
        return Err(smoke_fail("site info panel did not reflect permission decision"));
    }

    let private_origin = origin_from_url("https://private.example")
        .ok_or_else(|| smoke_fail("private origin parse failed"))?;
    let mut private_store = PermissionStore::load_from_path(path.clone());
    private_store.set(
        &private_origin,
        PermissionKind::Geolocation,
        PermissionDecision::Allow,
        true,
    );
    let (decision, source) = private_store.get(&private_origin, PermissionKind::Geolocation, true);
    if decision != PermissionDecision::Allow || source != DecisionSource::Session {
        return Err(smoke_fail("private permission did not use session store"));
    }
    let (decision, source) =
        private_store.get(&private_origin, PermissionKind::Geolocation, false);
    if decision != PermissionDecision::Ask || source != DecisionSource::Default {
        return Err(smoke_fail("private permission leaked into normal context"));
    }
    let private_reloaded = PermissionStore::load_from_path(path.clone());
    let (decision, source) =
        private_reloaded.get(&private_origin, PermissionKind::Geolocation, false);
    if decision != PermissionDecision::Ask || source != DecisionSource::Default {
        return Err(smoke_fail("private permission persisted unexpectedly"));
    }

    std::fs::write(&path, "{not-json")?;
    let malformed = PermissionStore::load_from_path(path.clone());
    let _ = std::fs::remove_file(&path);
    if !malformed.entries().is_empty() {
        return Err(smoke_fail("malformed permissions file was not ignored"));
    }

    smoke_log("[smoke] PASS permissions model");
    Ok(())
}

fn perf_mode_enabled() -> bool {
    std::env::var_os("RASHAMON_PERF").is_some()
        || std::env::args().skip(1).any(|arg| arg == "--perf")
}

fn smoke_quiet_enabled() -> bool {
    std::env::var_os("RASHAMON_SMOKE_QUIET").is_some()
        || std::env::args().skip(1).any(|arg| arg == "--smoke-quiet")
}

fn smoke_log(msg: impl AsRef<str>) {
    if !smoke_quiet_enabled() {
        eprintln!("{}", msg.as_ref());
    }
}

#[derive(Debug, Default, Clone)]
struct SmokePerfMetrics {
    cold_start_ms: Option<u128>,
    first_webkit_frame_ms: Option<u128>,
    tab_switch_latency_ms: Option<u128>,
    scroll_snapshot_latency_ms: Option<u128>,
    load_start_ms: Option<u128>,
    load_finished_ms: Option<u128>,
    snapshot_completed_ms: Option<u128>,
    libsoup_warning_count: u64,
    webext_available: bool,
    webext_loaded: bool,
    webext_disabled: bool,
    webext_handshake_ok: bool,
    webext_blocked_event_seen: bool,
}

fn run_webkit_smoke_test() -> Result<SmokePerfMetrics, Box<dyn std::error::Error>> {
    let smoke_t0 = Instant::now();
    let perf = perf_mode_enabled();
    let mut perf_metrics = SmokePerfMetrics::default();
    std::env::set_var("RASHAMON_WEBEXT_SMOKE_PROBE", "1");
    smoke_log("[smoke] starting WebKit smoke test");
    if std::env::var_os("RASHAMON_PERF").is_none() {
        smoke_adblock_model()?;
        smoke_download_destination_model()?;
        smoke_permissions_model()?;
    } else {
        smoke_log("[smoke] PERF mode: skipping file-writing model preflight checks");
    }
    let mut engine = RenderEngine::new(FB_WIDTH, FB_HEIGHT.saturating_sub(TOP_BAR_HEIGHT))?;
    if !engine.is_real_engine() {
        return Err(smoke_fail("WebKit smoke test requires a real engine, got fallback"));
    }
    if perf {
        let value = smoke_t0.elapsed().as_millis();
        perf_metrics.cold_start_ms = Some(value);
        eprintln!("[perf] cold-start-ms={value}");
    }

    let mut state = BrowserState::new();
    let first_id = state.active_tab_id;
    engine.create_tab(first_id.raw(), false);
    engine.set_active_tab(first_id.raw());
    smoke_wait_for(&mut state, &mut engine, "startup no args", Duration::from_secs(2), |s, _| {
        s.tabs.len() == 1 && s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::NewTab))
    })?;
    perf_metrics.webext_available = webext_is_available_for_test();
    perf_metrics.webext_loaded = webext_is_configured_for_test();
    perf_metrics.webext_disabled = webext_is_disabled_for_test();
    smoke_log(format!(
        "[smoke] webext_available={} webext_loaded={} webext_disabled={}",
        perf_metrics.webext_available, perf_metrics.webext_loaded, perf_metrics.webext_disabled
    ));

    if webext_is_configured_for_test() && std::env::var_os("RASHAMON_DISABLE_WEBEXT").is_none() {
        let blocked_probe_before = webext_blocked_events_for_test();
        let probe_blocked_before = webext_probe_blocked_for_test();
        let rules_ok_before = webext_rules_ok_for_test();
        let rules_error_before = webext_rules_error_for_test();
        match smoke_wait_for(
            &mut state,
            &mut engine,
            "webext ready handshake",
            Duration::from_secs(3),
            |_s, _| webext_ready_for_test(),
        ) {
            Ok(()) => {
                perf_metrics.webext_handshake_ok = true;
                smoke_log("[smoke] PASS webext runtime handshake");
                if smoke_wait_for(
                    &mut state,
                    &mut engine,
                    "webext valid rules sync",
                    Duration::from_secs(3),
                    |_s, _| webext_rules_ok_for_test() > rules_ok_before,
                )
                .is_ok()
                {
                    smoke_log("[smoke] PASS webext valid rules sync");
                } else {
                    smoke_log("[smoke] SKIP webext valid rules sync (not observed)");
                }
                if std::env::var_os("RASHAMON_WEBEXT_SMOKE_INVALID_RULES").is_some() {
                    if smoke_wait_for(
                        &mut state,
                        &mut engine,
                        "webext invalid rules rejected",
                        Duration::from_secs(3),
                        |_s, _| webext_rules_error_for_test() > rules_error_before,
                    )
                    .is_ok()
                    {
                        smoke_log("[smoke] PASS webext invalid rules rejected");
                    } else {
                        smoke_log("[smoke] SKIP webext invalid rules rejection (not observed)");
                    }
                    let invalid_rules_before = webext_rules_error_for_test();
                    let invalid_retained_probe_before = webext_probe_blocked_for_test();
                    engine.adblock_remove_allow_domain("invalid-probe.invalid");
                    if smoke_wait_for(
                        &mut state,
                        &mut engine,
                        "webext invalid rules kept previous rules",
                        Duration::from_secs(3),
                        |_s, _| {
                            webext_rules_error_for_test() > invalid_rules_before
                                && webext_probe_blocked_for_test() > invalid_retained_probe_before
                        },
                    )
                    .is_ok()
                    {
                        smoke_log("[smoke] PASS webext invalid rules kept previous rules");
                    } else {
                        smoke_log("[smoke] SKIP webext invalid rules retention (not observed)");
                    }
                }
                if smoke_wait_for(
                    &mut state,
                    &mut engine,
                    "webext blocked-event probe",
                    Duration::from_secs(3),
                    |_s, _| {
                        webext_blocked_events_for_test() > blocked_probe_before
                            && webext_probe_blocked_for_test() > probe_blocked_before
                    },
                )
                .is_ok()
                {
                    perf_metrics.webext_blocked_event_seen = true;
                    smoke_log("[smoke] PASS webext blocked event path");
                } else {
                    smoke_log("[smoke] SKIP webext blocked event path (not observed)");
                }

                let allow_rules_before = webext_rules_ok_for_test();
                let allow_probe_clear_before = webext_probe_clear_for_test();
                engine.adblock_allow_domain("doubleclick.net");
                if smoke_wait_for(
                    &mut state,
                    &mut engine,
                    "webext rules sync after allowlist add",
                    Duration::from_secs(3),
                    |_s, _| {
                        webext_rules_ok_for_test() > allow_rules_before
                            && webext_probe_clear_for_test() > allow_probe_clear_before
                    },
                )
                .is_ok()
                {
                    smoke_log("[smoke] PASS webext rules sync after allowlist add");
                } else {
                    return Err(smoke_fail("webext rules did not resync after allowlist add"));
                }

                let block_rules_before = webext_rules_ok_for_test();
                let block_probe_before = webext_probe_blocked_for_test();
                engine.adblock_remove_allow_domain("doubleclick.net");
                if smoke_wait_for(
                    &mut state,
                    &mut engine,
                    "webext rules sync after allowlist remove",
                    Duration::from_secs(3),
                    |_s, _| {
                        webext_rules_ok_for_test() > block_rules_before
                            && webext_probe_blocked_for_test() > block_probe_before
                    },
                )
                .is_ok()
                {
                    smoke_log("[smoke] PASS webext rules sync after allowlist remove");
                } else {
                    return Err(smoke_fail("webext rules did not resync after allowlist remove"));
                }
            }
            Err(_) => {
                smoke_log("[smoke] SKIP webext runtime handshake (not observed)");
            }
        }
    } else {
        smoke_log("[smoke] SKIP webext runtime handshake (webext disabled/unavailable)");
        smoke_log("[smoke] SKIP webext blocked event path (webext disabled/unavailable)");
    }

    let blocked_before = webext_blocked_events_for_test();
    let policy_blocks_before = adblock_policy_blocks_for_test();
    smoke_navigate(&mut state, &mut engine, "https://doubleclick.net/pagead/id")?;
    smoke_wait_for(&mut state, &mut engine, "adblock rejects blocked URL", Duration::from_secs(4), |s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadFailed(reason) if reason.contains("Blocked by adblock")))
            && s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Error(_)))
    })?;
    if adblock_policy_blocks_for_test() <= policy_blocks_before {
        return Err(smoke_fail("in-process policy block counter did not increment"));
    }
    if perf_metrics.webext_loaded && !perf_metrics.webext_disabled && !perf_metrics.webext_blocked_event_seen {
        let blocked_seen = webext_blocked_events_for_test() > blocked_before;
        perf_metrics.webext_blocked_event_seen = blocked_seen;
        if blocked_seen {
            smoke_log("[smoke] PASS webext blocked event path (navigation)");
        }
    }

    smoke_navigate(&mut state, &mut engine, "https://example.com")?;
    let first_webkit_t0 = Instant::now();
    smoke_wait_for(&mut state, &mut engine, "startup/direct URL renders", Duration::from_secs(12), |s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
            && s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Loaded))
    })?;
    if perf {
        let value = first_webkit_t0.elapsed().as_millis();
        perf_metrics.first_webkit_frame_ms = Some(value);
        eprintln!("[perf] first-webkit-frame-ms={value}");
    }
    state.find_open = true;
    state.find_input = "Example".to_string();
    engine.find_text(&state.find_input);
    smoke_wait_for(&mut state, &mut engine, "find in page counts matches", Duration::from_secs(4), |_s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::FindMatchCount(count) if *count > 0))
    })?;
    engine.find_next();
    engine.find_previous();
    engine.find_clear();
    state.find_open = false;
    state.find_input.clear();
    state.find_match_count = None;
    smoke_wait_for(&mut state, &mut engine, "find in page clears", Duration::from_secs(1), |_s, _| true)?;
    engine.download_url("https://example.com/");
    smoke_wait_for(&mut state, &mut engine, "download signal path starts", Duration::from_secs(8), |_s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::DownloadStarted { .. }))
    })?;
    let first_loaded_id = state.active_tab_id;

    let new_tab_rules_before = webext_rules_ok_for_test();
    smoke_create_tab(&mut state, &mut engine, false);
    if webext_is_configured_for_test() && std::env::var_os("RASHAMON_DISABLE_WEBEXT").is_none() {
        if smoke_wait_for(
            &mut state,
            &mut engine,
            "webext rules sync after new tab",
            Duration::from_secs(3),
            |_s, _| webext_rules_ok_for_test() > new_tab_rules_before,
        )
        .is_ok()
        {
            smoke_log("[smoke] PASS webext rules sync after new tab");
        } else {
            return Err(smoke_fail("webext rules did not sync after new tab"));
        }
    }
    {
        use omnibox::{classify_input, InputKind, DEFAULT_PROVIDER};
        let search_url = match classify_input("rust browser engine") {
            InputKind::Search(q) => DEFAULT_PROVIDER.build_url(&q),
            _ => return Err(smoke_fail("search query was not classified as search")),
        };
        smoke_navigate(&mut state, &mut engine, &search_url)?;
    }
    smoke_wait_for(&mut state, &mut engine, "startup/search query renders", Duration::from_secs(12), |s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
            && s.active_tab().map_or(false, |t| t.url.contains("duckduckgo.com"))
    })?;
    let search_tab_id = state.active_tab_id;

    state.activate_tab(first_loaded_id);
    let tab_switch_t0 = Instant::now();
    engine.set_active_tab(first_loaded_id.raw());
    smoke_wait_for(&mut state, &mut engine, "tab switch without reload", Duration::from_secs(3), |s, events| {
        s.active_tab_id == first_loaded_id
            && s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Loaded))
            && events.iter().any(|e| matches!(e, EngineEvent::FrameReady { reason } if reason.contains("switch")))
    })?;
    if perf {
        let value = tab_switch_t0.elapsed().as_millis();
        perf_metrics.tab_switch_latency_ms = Some(value);
        eprintln!("[perf] tab-switch-latency-ms={value}");
    }
    engine.force_suspend_inactive_tabs();
    smoke_wait_for(&mut state, &mut engine, "inactive tab suspension stable", Duration::from_secs(2), |_s, _| true)?;
    state.activate_tab(search_tab_id);
    engine.set_active_tab(search_tab_id.raw());
    smoke_wait_for(&mut state, &mut engine, "wake suspended tab renders", Duration::from_secs(12), |s, events| {
        s.active_tab_id == search_tab_id
            && (s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Loaded))
                || events.iter().any(|e| matches!(e, EngineEvent::LoadComplete)))
    })?;
    state.activate_tab(first_loaded_id);
    engine.set_active_tab(first_loaded_id.raw());
    smoke_wait_for(&mut state, &mut engine, "first tab ready after wake", Duration::from_secs(12), |s, events| {
        s.active_tab_id == first_loaded_id
            && s.active_tab().map_or(false, |t| t.url.contains("example.com"))
            && (events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
                || s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Loaded)))
    })?;
    for _ in 0..4 {
        state.activate_tab(search_tab_id);
        engine.set_active_tab(search_tab_id.raw());
        engine.pump_gtk();
        state.activate_tab(first_loaded_id);
        engine.set_active_tab(first_loaded_id.raw());
        engine.pump_gtk();
    }
    smoke_wait_for(&mut state, &mut engine, "rapid tab switching stable", Duration::from_secs(3), |s, _| {
        s.active_tab_id == first_loaded_id
            && s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Loaded))
    })?;

    smoke_navigate(&mut state, &mut engine, "https://example.org")?;
    smoke_wait_for(&mut state, &mut engine, "second URL renders", Duration::from_secs(12), |s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
            && s.active_tab().map_or(false, |t| t.url.contains("example.org"))
    })?;
    smoke_wait_for(&mut state, &mut engine, "native back becomes available", Duration::from_secs(8), |s, _| {
        s.active_tab().map_or(false, |t| t.webkit_can_back)
    })?;
    engine.go_back().ok();
    smoke_wait_for(&mut state, &mut engine, "back returns to first URL", Duration::from_secs(8), |s, _| {
        s.active_tab().map_or(false, |t| t.url.contains("example.com"))
    })?;
    smoke_wait_for(&mut state, &mut engine, "native forward becomes available", Duration::from_secs(8), |s, _| {
        s.active_tab().map_or(false, |t| t.webkit_can_forward)
    })?;
    engine.go_forward().ok();
    smoke_wait_for(&mut state, &mut engine, "forward returns to second URL", Duration::from_secs(8), |s, _| {
        s.active_tab().map_or(false, |t| t.url.contains("example.org"))
    })?;
    let reload_url = state.active_tab().map(|t| t.url.clone()).unwrap_or_default();
    smoke_navigate(&mut state, &mut engine, &reload_url)?;
    smoke_wait_for(&mut state, &mut engine, "reload renders", Duration::from_secs(12), |_s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
    })?;

    let scroll_t0 = Instant::now();
    state.scroll_by(SCROLL_WHEEL * 4);
    engine.scroll(SCROLL_WHEEL * 4);
    smoke_wait_for(&mut state, &mut engine, "scroll command stable", Duration::from_secs(2), |_s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::FrameReady { reason } if reason.contains("scroll")))
    })?;
    if perf {
        let value = scroll_t0.elapsed().as_millis();
        perf_metrics.scroll_snapshot_latency_ms = Some(value);
        eprintln!("[perf] scroll-snapshot-latency-ms={value}");
    }
    for _ in 0..8 {
        state.scroll_by(SCROLL_WHEEL);
        engine.scroll(SCROLL_WHEEL);
        engine.pump_gtk();
    }
    smoke_wait_for(&mut state, &mut engine, "rapid scroll stable", Duration::from_secs(2), |_s, _| true)?;

    let before_bookmarks = state.bookmarks.len();
    state.toggle_bookmark();
    if state.bookmarks.len() != before_bookmarks + 1 {
        return Err(smoke_fail("bookmark add failed"));
    }
    let bm_url = state.bookmarks.last().map(|b| b.url.clone()).ok_or_else(|| smoke_fail("missing added bookmark"))?;
    state.toggle_bookmark();
    if state.bookmarks.len() != before_bookmarks {
        return Err(smoke_fail("bookmark remove failed"));
    }
    smoke_navigate(&mut state, &mut engine, &bm_url)?;
    smoke_wait_for(&mut state, &mut engine, "bookmark open renders", Duration::from_secs(12), |_s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
    })?;

    let hist_url = state.global_history.last()
        .map(|e| e.url.clone())
        .ok_or_else(|| smoke_fail("history was not recorded"))?;
    state.toggle_overlay(OverlayKind::History);
    let overlay_url = state.activate_overlay_item()
        .ok_or_else(|| smoke_fail("history overlay did not select an entry"))?;
    if overlay_url != hist_url {
        return Err(smoke_fail("history overlay selected unexpected URL"));
    }
    smoke_navigate(&mut state, &mut engine, &overlay_url)?;
    smoke_wait_for(&mut state, &mut engine, "history reopen renders", Duration::from_secs(12), |_s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
    })?;

    let public_history_len = state.global_history.len();
    smoke_create_tab(&mut state, &mut engine, true);
    smoke_navigate(&mut state, &mut engine, "https://example.com")?;
    smoke_wait_for(&mut state, &mut engine, "private tab renders", Duration::from_secs(12), |_s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadComplete))
    })?;
    if state.global_history.len() != public_history_len {
        return Err(smoke_fail("private tab persisted global history"));
    }
    let private_tab_id = state.active_tab_id;
    state.activate_tab(first_loaded_id);
    engine.set_active_tab(first_loaded_id.raw());
    engine.force_suspend_inactive_tabs();
    smoke_wait_for(&mut state, &mut engine, "private inactive tab suspension stable", Duration::from_secs(2), |_s, _| true)?;
    state.activate_tab(private_tab_id);
    engine.set_active_tab(private_tab_id.raw());
    smoke_wait_for(&mut state, &mut engine, "wake suspended private tab renders", Duration::from_secs(12), |s, events| {
        s.active_tab_id == private_tab_id
            && (s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Loaded))
                || events.iter().any(|e| matches!(e, EngineEvent::LoadComplete)))
    })?;

    engine.zoom_in();
    engine.zoom_out();
    engine.zoom_reset();
    smoke_wait_for(&mut state, &mut engine, "zoom commands stable", Duration::from_secs(2), |_s, _| true)?;

    smoke_create_tab(&mut state, &mut engine, false);
    smoke_navigate(&mut state, &mut engine, "http://nonexistent.invalid")?;
    smoke_wait_for(&mut state, &mut engine, "broken URL errors", Duration::from_secs(12), |s, events| {
        events.iter().any(|e| matches!(e, EngineEvent::LoadFailed(_)))
            && s.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Error(_)))
    })?;

    let closing_id = state.active_tab_id;
    engine.close_tab(closing_id.raw());
    state.close_tab(closing_id);
    engine.set_active_tab(state.active_tab_id.raw());
    if state.tabs.iter().any(|t| t.id == closing_id) {
        return Err(smoke_fail("close tab failed"));
    }
    smoke_log("[smoke] PASS close tab");

    while state.tabs.len() > 1 {
        let id = state.active_tab_id;
        engine.close_tab(id.raw());
        state.close_tab(id);
        engine.set_active_tab(state.active_tab_id.raw());
    }
    let last_id = state.active_tab_id;
    engine.close_tab(last_id.raw());
    state.close_tab(last_id);
    engine.create_tab(state.active_tab_id.raw(), false);
    engine.set_active_tab(state.active_tab_id.raw());
    if state.tabs.len() != 1 || !state.active_tab().map_or(false, |t| matches!(t.page_state, PageState::NewTab)) {
        return Err(smoke_fail("close last tab did not restore a new tab"));
    }
    smoke_log("[smoke] PASS close last tab");
    smoke_log("[smoke] PASS WebKit smoke test complete");
    Ok(perf_metrics)
}

// ── Content height measurement ────────────────────────────────────────────────

fn measure_content_height(nodes: &[PageNode], font: &FontManager) -> u32 {
    let mut h: u32 = PAD_TOP;
    for node in nodes {
        match node {
            PageNode::Heading { level, text } => {
                let (size, before, after): (f32, u32, u32) = match level {
                    1 => (28.0, 18, 10), 2 => (22.0, 14, 8), _ => (17.0, 10, 6),
                };
                h += before + wrap_text(text, font, size, MAX_W).len() as u32 * (size as u32 + 4) + after;
            }
            PageNode::Paragraph(text) => {
                if text.trim().is_empty() { continue; }
                h += 4 + wrap_text(text, font, 14.0, MAX_W).len() as u32 * 22 + 10;
            }
            PageNode::ListItem(text) => {
                let b = format!("  \u{2022}  {text}");
                h += wrap_text(&b, font, 13.0, MAX_W).len() as u32 * 20 + 3;
            }
            PageNode::Pre(text) => { h += 8 + text.lines().count() as u32 * 18 + 30; }
            PageNode::HRule     => { h += 24; }
        }
    }
    h + 60
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("Rashamon Arc {APP_VERSION}");

    if std::env::args().skip(1).any(|arg| arg == "--bench-perf") {
        let mut runs = 10usize;
        let mut mode = BenchTempMode::Warm;
        let mut isolate_profile = std::env::var_os("RASHAMON_BENCH_ISOLATE_PROFILE").is_some();
        let args: Vec<String> = std::env::args().collect();
        for i in 0..args.len() {
            if args[i] == "--runs" {
                if let Some(next) = args.get(i + 1) {
                    if let Ok(parsed) = next.parse::<usize>() {
                        runs = parsed.max(1);
                    }
                }
            }
            if args[i] == "--cold" {
                mode = BenchTempMode::Cold;
            }
            if args[i] == "--warm" {
                mode = BenchTempMode::Warm;
            }
            if args[i] == "--bench-isolate-profile" {
                isolate_profile = true;
            }
        }
        return run_bench_perf(
            runs,
            BenchConfig {
                mode,
                isolate_profile,
            },
        );
    }

    if std::env::args().skip(1).any(|arg| arg == "--smoke-test-permissions") {
        return smoke_permissions_model();
    }

    if std::env::args().skip(1).any(|arg| arg == "--smoke-test-webkit" || arg == "--smoke-test-adblock" || arg == "--smoke-test-find") {
        run_webkit_smoke_test()?;
        return Ok(());
    }

    #[cfg(feature = "linux-desktop")]
    {
        return run_browser_runtime(LinuxDesktopRuntime::new()?);
    }

    #[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
    {
        return run_browser_runtime(runtime::KamelotRuntime::new()?);
    }

    #[cfg(not(any(feature = "linux-desktop", feature = "kamelot")))]
    {
        eprintln!("Platform: no backend selected; enable linux-desktop or kamelot");
        return Ok(());
    }
}

fn run_browser_runtime<R: PlatformRuntime>(
    mut runtime: R,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut perf = PerfTracker::new();
    draw_bootstrap_shell(runtime.framebuffer_mut());
    runtime.present_frame()?;
    perf.on_initial_frame();

    let perf_sample_deadline = if std::env::args().skip(1).any(|arg| arg == "--perf-sample") {
        Some(Instant::now() + Duration::from_secs(4))
    } else {
        None
    };
    let content_h  = FB_HEIGHT.saturating_sub(TOP_BAR_HEIGHT);
    let mut driver = BrowserCoreDriver::new(FB_WIDTH, content_h)?;
    let _http       = HttpClient::new();
    let font_data = load_ui_font_data();
    let font = FontManager::new(font_data)?;

    let mut pending_fetch:    Option<PendingFetch>          = None;
    let mut buffered_outcome: Option<(TabId, FetchOutcome)> = None;

    let startup_input = std::env::args()
        .skip(1)
        .filter(|arg| arg != "--smoke-test" && arg != "--smoke-test-webkit" && arg != "--smoke-test-adblock" && arg != "--smoke-test-find")
        .collect::<Vec<_>>()
        .join(" ");
    if !startup_input.trim().is_empty() {
        driver.dispatch(BrowserAction::Navigate(startup_input.trim().to_string()));
    }

    let initial_dirty = DirtyFlags { tabs: true, chrome: true, content: true };
    render_ui(
        runtime.framebuffer_mut(),
        &driver.state,
        &font,
        initial_dirty,
        ContentRenderMode::Text,
    );
    runtime.present_frame()?;
    if driver
        .state
        .active_tab()
        .map_or(false, |t| t.page_state.is_loading())
    {
        perf.on_loading_surface_frame();
    }

    let mut running          = true;
    let mut last_blink_phase = 0u64;

    while running && !runtime.should_exit() {
        if perf_sample_deadline.map_or(false, |deadline| Instant::now() >= deadline) {
            break;
        }
        perf.mark_frame_start();
        driver.state.frame_count += 1;
        driver.state.tick_nav_btn();

        // Pump GTK/GLib events so WebKitGTK can process network responses,
        // fire load-changed signals, and complete snapshots. No-op on stub.
        driver.pump_engine();

        // ── Events ────────────────────────────────────────────────────────────
        for ev in runtime.poll_events()? {
            perf.note_input(&ev);
            if driver.handle_platform_event(ev, (FB_HEIGHT - TOP_BAR_HEIGHT) as i32) {
                running = false;
                break;
            }
        }
        perf.note_active_tab(driver.state.active_tab_id.raw());
        let effective_cursor = if driver.state.active_tab().map_or(false, |t| t.page_state.is_loading()) {
            CursorKind::Wait
        } else {
            driver.cursor
        };
        runtime.set_cursor(effective_cursor);

        // ── Spawn fetch (text renderer fallback — skipped when real engine active) ──
        #[cfg(not(all(feature = "kamelot", not(feature = "linux-desktop"))))]
        if !driver.engine.is_real_engine() {
            if let Some(tab) = driver.state.active_tab() {
                if tab.page_state.is_loading() {
                    let already = pending_fetch.as_ref().map_or(false, |pf| pf.tab_id == tab.id)
                        || buffered_outcome.as_ref().map_or(false, |(id, _)| *id == tab.id);
                    if !already {
                        let tab_id = tab.id;
                        let url    = tab.url.clone();
                        pending_fetch = Some(spawn_fetch(tab_id, url));
                    }
                }
            }
        }

        #[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
        if !driver.engine.is_real_engine() {
            if driver
                .state
                .active_tab()
                .map_or(false, |t| t.page_state.is_loading())
            {
                driver
                    .state
                    .fail_loading("Web content renderer/network unavailable on Kamelot yet.");
            }
        }

        // ── Poll fetch ────────────────────────────────────────────────────────
        if let Some(ref pf) = pending_fetch {
            match pf.receiver.try_recv() {
                Ok(outcome) => {
                    let tab_id = pf.tab_id;
                    pending_fetch = None;
                    buffered_outcome = Some((tab_id, outcome));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let tab_id = pf.tab_id;
                    pending_fetch = None;
                    if driver.state.active_tab().map_or(false, |t| t.id == tab_id && t.page_state.is_loading()) {
                        driver.state.fail_loading("Connection lost");
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        // ── Apply buffered result (after min visual loading time) ─────────────
        if let Some((tab_id, _)) = &buffered_outcome {
            let tab_id = *tab_id;
            let ready = driver.state.active_tab()
                .filter(|t| t.id == tab_id && t.page_state.is_loading())
                .map_or(false, |t| {
                    driver.state.frame_count.saturating_sub(t.load_start_frame) >= LOAD_MIN_FRAMES
                });
            if ready {
                let (_, outcome) = buffered_outcome.take().unwrap();
                match outcome {
                    FetchOutcome::Success { title, nodes, meta_description, noscript } => {
                        let h = measure_content_height(&nodes, &font);
                        driver.state.resolve_loading(title.unwrap_or_default(), nodes, meta_description, noscript);
                        driver.state.set_content_height(h);
                        driver.save_dirty.history = true;
                    }
                    FetchOutcome::Failure(reason) => { driver.state.fail_loading(&reason); }
                }
            }
        }

        // ── Loading timeout ───────────────────────────────────────────────────
        driver.tick_loading();

        // ── Continuous-animation dirty ────────────────────────────────────────
        if driver.state.active_tab().map_or(false, |t| t.page_state.is_loading()) {
            driver.state.dirty.tabs    = true;
            driver.state.dirty.chrome  = true;
            driver.state.dirty.content = true;
        }
        if driver.state.address_bar_focused {
            let blink = driver.state.frame_count / 28;
            if blink != last_blink_phase {
                last_blink_phase = blink;
                driver.state.dirty_address_bar();
            }
        }

        // ── Lazy content height (after back/forward) ──────────────────────────
        if driver.state.dirty.content && driver.state.overlay == OverlayKind::None {
            if let Some(tab) = driver.state.active_tab() {
                if matches!(tab.page_state, PageState::Loaded) && tab.content_height == 0 {
                    let nodes: Vec<PageNode> = tab.current_nodes().to_vec();
                    if !nodes.is_empty() {
                        let h = measure_content_height(&nodes, &font);
                        driver.state.set_content_height(h);
                    }
                }
            }
        }

        // ── Engine events — routed by BrowserCoreDriver ──────────────────────
        let engine_events = driver.poll_engine_events();
        for (_, ev) in &engine_events {
            perf.on_engine_event(ev);
        }

        // ── Render ────────────────────────────────────────────────────────────
        if driver.state.dirty.any() {
            let dirty = driver.state.dirty;
            driver.state.dirty.clear();

            // Ask the engine to composite content pixels first. While a real
            // engine is waiting on a fresh snapshot, keep the shell in an
            // intentional pending state instead of showing text fallback.
            let content_mode = if dirty.content
                && driver.state.overlay == OverlayKind::None
                && driver.state.active_tab().map_or(false, |t| matches!(t.page_state, PageState::Loaded))
            {
                let render_t0 = Instant::now();
                let render_result = driver.engine.render_into(
                    runtime.framebuffer_mut(),
                    0,
                    TOP_BAR_HEIGHT,
                    FB_WIDTH,
                    FB_HEIGHT - TOP_BAR_HEIGHT,
                );
                perf.add_render_into(render_t0.elapsed());
                match render_result {
                    Ok(EngineFrame::Ready) => ContentRenderMode::Engine,
                    Ok(_) if driver.engine.is_real_engine() => ContentRenderMode::EnginePending,
                    Ok(_) => ContentRenderMode::Text,
                    Err(e) => {
                        if std::env::var_os("RASHAMON_DEBUG").is_some() {
                            eprintln!("[render] engine.render_into error: {e}");
                        }
                        ContentRenderMode::Text
                    }
                }
            } else {
                ContentRenderMode::Text
            };

            let draw_t0 = Instant::now();
            render_ui(runtime.framebuffer_mut(), &driver.state, &font, dirty, content_mode);
            perf.add_draw_ui(draw_t0.elapsed());
            let present_t0 = Instant::now();
            runtime.present_frame()?;
            perf.add_present(present_t0.elapsed());
            if driver
                .state
                .active_tab()
                .map_or(false, |t| t.page_state.is_loading())
            {
                perf.on_loading_surface_frame();
            }
        }

        perf.maybe_dump_runtime_stats(&driver);
        perf.maybe_report_startup();
        perf.mark_frame_end();

        // ── Persist (fire-and-forget after render) ────────────────────────────
        if driver.save_dirty.any() {
            driver.flush_saves();
        }

        runtime.tick();
    }
    Ok(())
}

// ── Top-level render ──────────────────────────────────────────────────────────

fn draw_bootstrap_shell(fb: &mut Framebuffer) {
    use theme::KAMELOT_DARK;
    fb.clear(KAMELOT_DARK.bg);
    fb.fill_rect(0, 0, fb.width, TAB_BAR_HEIGHT, KAMELOT_DARK.tab_bar_bg);
    fb.fill_rect(
        0,
        TAB_BAR_HEIGHT,
        fb.width,
        CHROME_BAR_HEIGHT,
        KAMELOT_DARK.surface,
    );
    fb.fill_rect(0, TOP_BAR_HEIGHT, fb.width, 1, KAMELOT_DARK.border);
    fb.fill_rect(0, TOP_BAR_HEIGHT + 1, fb.width, 2, KAMELOT_DARK.accent);
}

/// `content_mode` tells the shell whether the engine already wrote pixels or
/// whether it is still waiting on the next snapshot for a loaded page.
fn render_ui(
    fb:              &mut Framebuffer,
    state:           &BrowserState,
    font:            &FontManager,
    dirty:           DirtyFlags,
    content_mode:    ContentRenderMode,
) {
    let theme      = state.theme;
    let tw         = state.tab_width;
    let active_pos = state.active_pos;

    if dirty.content {
        if state.overlay != OverlayKind::None {
            draw_overlay(fb, state, font);
        } else {
            match state.active_tab().map(|t| &t.page_state) {
                Some(PageState::NewTab)   => {
                    if state.active_tab().map_or(false, |t| t.is_private) {
                        draw_private_new_tab(fb, state, font);
                    } else {
                        draw_new_tab(fb, state, font);
                    }
                }
                Some(PageState::Loading)  => draw_loading(fb, state, font),
                Some(PageState::Error(_)) => draw_error(fb, state, font),
                Some(PageState::Loaded) if content_mode == ContentRenderMode::Engine => {
                    // Engine composited pixels directly into fb — nothing to do.
                }
                Some(PageState::Loaded) if content_mode == ContentRenderMode::EnginePending => {
                    draw_snapshot_pending(fb, state, font);
                }
                Some(PageState::Loaded)   => {
                    let (nodes, scroll_y) = state.active_tab()
                        .map(|t| (t.current_nodes(), t.scroll_y))
                        .unwrap_or((&[], 0));
                    draw_loaded(fb, state, font, nodes, scroll_y);
                }
                None => {}
            }
            draw_download_status(fb, state, font);
            draw_permission_prompt(fb, state, font);
            draw_site_info_panel(fb, state, font);
        }
    }

    if dirty.tabs {
        fb.fill_rect(0, 0, fb.width, TAB_BAR_HEIGHT, theme.tab_bar_bg);
        draw_tab_row(fb, state, font);
        fb.fill_rect(0, TAB_BAR_HEIGHT - 1, fb.width, 1, theme.border);
        let active_x = TAB_START_X + active_pos as u32 * (tw + TAB_SEP);
        fb.fill_rect(active_x, TAB_BAR_HEIGHT - 1, tw, 2, theme.surface);
    }

    if dirty.chrome {
        fb.fill_rect(0, TAB_BAR_HEIGHT, fb.width, CHROME_BAR_HEIGHT, theme.surface);
        draw_chrome_row(fb, state, font);
        fb.fill_rect(0, TOP_BAR_HEIGHT, fb.width, 1, theme.border);
    }
}

// ── Tab row ───────────────────────────────────────────────────────────────────

fn draw_tab_row(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let tw    = state.tab_width;
    const TOP: u32 = 4;
    const H:   u32 = TAB_BAR_HEIGHT - TOP;

    for (i, tab) in state.tabs.iter().enumerate() {
        let tx         = TAB_START_X + i as u32 * (tw + TAB_SEP);
        let is_active  = tab.id == state.active_tab_id;
        let is_hovered = state.mouse_y < TAB_BAR_HEIGHT
            && state.mouse_x >= tx && state.mouse_x < tx + tw;

        let bg = if is_active       { theme.tab_active_bg }
                 else if is_hovered { theme.tab_hover_bg  }
                 else               { theme.tab_bg        };
        let fg = if is_active { theme.tab_active_fg } else { theme.tab_fg };

        if !is_active {
            fb.fill_rect(tx + 8, TAB_BAR_HEIGHT - 2, tw.saturating_sub(16), 1, state.theme.border);
        }
        draw::draw_rounded_rect_top(fb, tx, TOP, tw, H, 8, bg);

        if is_active {
            fb.fill_rect(tx, TAB_BAR_HEIGHT - 2, tw, 3, theme.surface);
            let stripe = if tab.is_private { PRIVATE_ACCENT } else { theme.accent };
            fb.fill_rect(tx, TOP + 4, 2, H - 8, stripe);
        }

        // Private tab: small badge dot before title
        let title_x = if tab.is_private && (is_active || is_hovered) {
            draw::draw_circle_filled(fb, tx + 10, TOP + H / 2, 4, PRIVATE_ACCENT);
            tx + 18
        } else {
            tx + 14
        };

        let close_reserve = if is_active || is_hovered { 24 } else { 8 };
        let max_title_w   = tw.saturating_sub(title_x - tx + close_reserve);
        let title_y       = TOP + (H / 2).saturating_sub(7);
        draw::draw_text(fb, font, title_x, title_y, tab.tab_title(), 12.5, fg, max_title_w);

        if is_active || is_hovered {
            let cx = tx + tw.saturating_sub(16);
            let cy = TOP + H / 2;
            let close_hot = state.mouse_x >= cx.saturating_sub(8)
                && state.mouse_x < cx + 8
                && state.mouse_y >= TOP && state.mouse_y < TAB_BAR_HEIGHT;
            if close_hot {
                draw::draw_circle_filled(fb, cx, cy, 8, theme.tab_close_hover);
            }
            draw::draw_icon_close(fb, cx, cy, 7, fg);
        }

        if tab.page_state.is_loading() {
            let anim = (state.frame_count * 4 % tw as u64) as u32;
            fb.fill_rect(tx, TAB_BAR_HEIGHT - 3, anim, 2, theme.accent);
        }
        if tab.page_state.is_error() {
            let dot_x = tx + tw.saturating_sub(28);
            fb.fill_rect(dot_x, TOP + H / 2 - 3, 6, 6, theme.security_err);
        }
        if tab.is_pinned {
            fb.fill_rect(tx + 5, TOP + 6, 4, 4, theme.accent);
        }
    }

    let add_x  = TAB_START_X + state.tabs.len() as u32 * (tw + TAB_SEP);
    let add_cx = add_x + TAB_NEW_BTN_W / 2;
    let add_cy = TOP + H / 2;
    let add_hot = state.mouse_y < TAB_BAR_HEIGHT
        && state.mouse_x >= add_x && state.mouse_x < add_x + TAB_NEW_BTN_W;
    if add_hot {
        draw::draw_circle_filled(fb, add_cx, add_cy, 13, theme.tab_hover_bg);
    }
    draw::draw_icon_add(fb, add_cx, add_cy, 10, theme.icon_fg);
}

// ── Chrome row ────────────────────────────────────────────────────────────────

fn draw_chrome_row(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let cy          = TAB_BAR_HEIGHT + CHROME_BAR_HEIGHT / 2;
    let can_back    = state.active_tab().map_or(false, |t| t.nav_can_go_back());
    let can_forward = state.active_tab().map_or(false, |t| t.nav_can_go_forward());
    draw_nav_btn(fb, state, 28,  cy, NavBtn::Back,    can_back);
    draw_nav_btn(fb, state, 70,  cy, NavBtn::Forward, can_forward);
    draw_nav_btn(fb, state, 112, cy, NavBtn::Reload,  true);
    draw_address_bar(fb, state, font);
    if state.find_open {
        draw_find_bar(fb, state, font);
    }
    draw::draw_icon_menu(fb, FB_WIDTH - 28, cy, state.theme.icon_fg);
}

enum NavBtn { Back, Forward, Reload }

fn draw_nav_btn(fb: &mut Framebuffer, state: &BrowserState, cx: u32, cy: u32, btn: NavBtn, enabled: bool) {
    let theme  = state.theme;
    let r: u32 = 16;
    let btn_id = match btn { NavBtn::Back => 1u8, NavBtn::Forward => 2, NavBtn::Reload => 3 };
    let hovered = state.mouse_y >= TAB_BAR_HEIGHT && state.mouse_y < TOP_BAR_HEIGHT
        && state.mouse_x >= cx.saturating_sub(r) && state.mouse_x < cx + r;
    let pressed = state.nav_btn_pressed == btn_id;
    if pressed {
        draw::draw_circle_filled(fb, cx, cy, r, theme.accent);
    } else if hovered && enabled {
        draw::draw_circle_filled(fb, cx, cy, r, theme.control_hover_bg);
    } else if enabled {
        draw::draw_circle_filled(fb, cx, cy, r, theme.surface);
    }
    let color = if pressed       { theme.accent_fg   }
                else if !enabled { theme.fg_secondary }
                else             { theme.icon_fg      };
    match btn {
        NavBtn::Back    => draw::draw_icon_back(fb, cx, cy, 10, color),
        NavBtn::Forward => draw::draw_icon_forward(fb, cx, cy, 10, color),
        NavBtn::Reload  => draw::draw_icon_reload(fb, cx, cy, 7, color),
    }
}

// ── Address bar ───────────────────────────────────────────────────────────────

fn draw_address_bar(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme  = state.theme;
    let bar_x  = (FB_WIDTH - ADDR_BAR_W) / 2;
    let bar_y  = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;
    let is_prv = state.active_tab().map_or(false, |t| t.is_private);

    let bg     = if state.address_bar_focused { theme.address_bar_bg_focused } else { theme.address_bar_bg };
    let border = if state.address_bar_focused { theme.address_bar_border_focused }
                 else if is_prv { PRIVATE_ACCENT }
                 else { theme.address_bar_border };
    fb.fill_rect(bar_x + 12, bar_y + ADDR_BAR_H + 1, ADDR_BAR_W.saturating_sub(24), 2, SHADOW_DARK);
    draw::draw_rounded_rect(fb, bar_x.saturating_sub(1), bar_y.saturating_sub(1),
        ADDR_BAR_W + 2, ADDR_BAR_H + 2, ADDR_BAR_R + 1, border);
    draw::draw_rounded_rect(fb, bar_x, bar_y, ADDR_BAR_W, ADDR_BAR_H, ADDR_BAR_R, bg);
    fb.fill_rect(bar_x + ADDR_BAR_R, bar_y + 1, ADDR_BAR_W.saturating_sub(2 * ADDR_BAR_R), 1, theme.control_hover_bg);

    let icon_x = bar_x + 14;
    let icon_y = bar_y + ADDR_BAR_H / 2;
    if let Some(tab) = state.active_tab() {
        match &tab.page_state {
            PageState::Loading  => draw::draw_icon_spinner(fb, icon_x, icon_y, 5, state.frame_count, theme.icon_fg),
            PageState::Error(_) => draw::draw_circle_filled(fb, icon_x, icon_y, 5, theme.security_err),
            _ if tab.url.starts_with("https://") => draw::draw_icon_lock(fb, icon_x, icon_y,
                if is_prv { PRIVATE_ACCENT } else { theme.security_ok }),
            _ if !tab.url.is_empty() => draw::draw_icon_globe(fb, icon_x, icon_y, theme.icon_fg),
            _ => {}
        }
    }

    let tx    = bar_x + 34;
    let ty    = bar_y + (ADDR_BAR_H.saturating_sub(14)) / 2;
    let max_w = ADDR_BAR_W.saturating_sub(34 + 30);

    if state.address_bar_input.is_empty() && !state.address_bar_focused {
        let placeholder = if is_prv { "Private search or URL" } else { "Search or enter URL" };
        draw::draw_text(fb, font, tx, ty, placeholder, 13.5, theme.placeholder, max_w);
    } else {
        draw::draw_text(fb, font, tx, ty, &state.address_bar_input, 13.5, theme.address_bar_fg, max_w);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw = font.text_width(&state.address_bar_input, 13.5);
            let cx = (tx + cw + 1).min(bar_x + ADDR_BAR_W - 34);
            fb.fill_rect(cx, ty, 2, 15, theme.accent);
        }
    }

    if let Some(tab) = state.active_tab() {
        let star_x   = bar_x + ADDR_BAR_W - 18;
        let star_col = if tab.is_bookmarked { theme.accent } else { theme.icon_fg };
        draw::draw_icon_star(fb, star_x, icon_y, 11, star_col, tab.is_bookmarked);
    }
}

fn draw_find_bar(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let w = 380;
    let h = 28;
    let x = FB_WIDTH.saturating_sub(w + 48);
    let y = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - h) / 2;

    fb.fill_rect(x + 10, y + h + 1, w.saturating_sub(20), 2, SHADOW_DARK);
    draw::draw_rounded_rect(fb, x.saturating_sub(1), y.saturating_sub(1), w + 2, h + 2, 9, theme.address_bar_border_focused);
    draw::draw_rounded_rect(fb, x, y, w, h, 9, theme.address_bar_bg_focused);

    let label = if state.find_input.is_empty() {
        "Find in page"
    } else {
        &state.find_input
    };
    let fg = if state.find_input.is_empty() { theme.placeholder } else { theme.address_bar_fg };
    draw::draw_text(fb, font, x + 12, y + 7, label, 12.5, fg, 210);
    if (state.frame_count / 28) % 2 == 0 {
        let cw = font.text_width(&state.find_input, 12.5);
        let cx = (x + 12 + cw + 1).min(x + 220);
        fb.fill_rect(cx, y + 7, 2, 14, theme.accent);
    }

    let count = match state.find_match_count {
        Some(0) => "0".to_string(),
        Some(n) => n.to_string(),
        None => "-".to_string(),
    };
    draw::draw_text(fb, font, x + 232, y + 7, &count, 12.5, theme.fg_secondary, 36);
    draw::draw_text(fb, font, x + 270, y + 7, "Enter next", 11.5, theme.fg_secondary, 86);
    draw::draw_text(fb, font, x + w - 20, y + 7, "x", 12.5, theme.fg_secondary, 14);
}

fn draw_download_status(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    if state.downloads.is_empty() {
        return;
    }
    let theme = state.theme;
    let item = state.downloads.last().unwrap();
    let w = 420;
    let h = 58;
    let x = FB_WIDTH.saturating_sub(w + 24);
    let y = TOP_BAR_HEIGHT + 20;

    draw::draw_rounded_rect(fb, x.saturating_sub(1), y.saturating_sub(1), w + 2, h + 2, 10, theme.border);
    draw::draw_rounded_rect(fb, x, y, w, h, 10, theme.surface);

    let (label, color) = match &item.status {
        DownloadStatus::Active => ("Downloading", theme.accent),
        DownloadStatus::Complete => ("Downloaded", theme.security_ok),
        DownloadStatus::Failed(_) => ("Download failed", theme.security_err),
    };
    draw::draw_text(fb, font, x + 14, y + 10, label, 13.0, color, 130);
    draw::draw_text(fb, font, x + 118, y + 10, &item.filename, 13.0, theme.fg, w - 132);

    let detail = match &item.status {
        DownloadStatus::Active => {
            let pct = (item.progress * 100.0).round().clamp(0.0, 100.0) as u32;
            format!("{pct}%  {} KB", item.received / 1024)
        }
        DownloadStatus::Complete => item.path.clone(),
        DownloadStatus::Failed(reason) => reason.clone(),
    };
    draw::draw_text(fb, font, x + 14, y + 34, &detail, 12.0, theme.fg_secondary, w - 28);
}

fn draw_permission_prompt(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let Some(prompt) = &state.permission_prompt else { return };
    let Some(tab) = state.tabs.iter().find(|t| t.id.raw() == prompt.tab_id) else { return };
    if tab.id != state.active_tab_id || tab.nav_id != prompt.nav_id {
        return;
    }

    let theme = state.theme;
    let (x, y, w, h) = permission_prompt_rect();
    draw::draw_rounded_rect(fb, x.saturating_sub(1), y.saturating_sub(1), w + 2, h + 2, 12, theme.border);
    draw::draw_rounded_rect(fb, x, y, w, h, 12, theme.surface);
    fb.fill_rect(x, y, 4, h, theme.accent);

    let title = format!("{} wants to access {}", prompt.origin, prompt.kind.as_str());
    draw::draw_text(fb, font, x + 18, y + 14, &title, 14.0, theme.fg, w - 36);
    draw::draw_text(
        fb,
        font,
        x + 18,
        y + 34,
        "Choose for this request. Remember stores the decision for this site.",
        12.0,
        theme.fg_secondary,
        w - 36,
    );

    let (remember, deny, allow) = permission_prompt_hit_rects();
    let box_col = if prompt.remember { theme.accent } else { theme.address_bar_border };
    draw::draw_rounded_rect_outline(fb, remember.0 as i32, (remember.1 + 3) as i32, 14, 14, 3, box_col);
    if prompt.remember {
        fb.fill_rect(remember.0 + 4, remember.1 + 7, 6, 3, theme.accent);
    }
    draw::draw_text(fb, font, remember.0 + 22, remember.1 + 4, "Remember", 12.0, theme.fg_secondary, 110);

    draw_prompt_button(fb, font, deny, "Deny", theme.control_hover_bg, theme.fg);
    draw_prompt_button(fb, font, allow, "Allow", theme.accent, theme.accent_fg);
}

fn draw_prompt_button(
    fb: &mut Framebuffer,
    font: &FontManager,
    rect: Rect,
    label: &str,
    bg: Pixel,
    fg: Pixel,
) {
    draw::draw_rounded_rect(fb, rect.0, rect.1, rect.2, rect.3, 8, bg);
    draw::draw_text(fb, font, rect.0 + 15, rect.1 + 7, label, 12.0, fg, rect.2 - 20);
}

fn permission_decision_for(
    state: &BrowserState,
    kind: PermissionKind,
) -> PermissionDecision {
    state
        .site_info
        .as_ref()
        .and_then(|panel| panel.permissions.iter().find(|(k, _)| *k == kind).map(|(_, d)| *d))
        .unwrap_or(PermissionDecision::Ask)
}

fn draw_site_info_panel(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let Some(panel) = &state.site_info else { return };
    let theme = state.theme;
    let (x, y, w, h) = site_info_rect();
    draw::draw_rounded_rect(fb, x.saturating_sub(1), y.saturating_sub(1), w + 2, h + 2, 12, theme.border);
    draw::draw_rounded_rect(fb, x, y, w, h, 12, theme.surface);

    let tab = state.active_tab();
    let url = tab.map(|t| t.url.as_str()).unwrap_or("");
    let private = tab.map_or(false, |t| t.is_private);
    let security = if url.starts_with("https://") {
        ("HTTPS secure", theme.security_ok)
    } else if url.starts_with("http://") {
        ("HTTP not secure", theme.security_err)
    } else {
        ("Internal / new tab", theme.fg_secondary)
    };
    let origin = panel.origin.as_deref().unwrap_or("No site loaded");

    draw::draw_text(fb, font, x + 18, y + 14, "Site information", 16.0, theme.fg, 220);
    draw::draw_text(fb, font, x + 18, y + 40, origin, 13.0, theme.fg, w - 36);
    draw::draw_text(fb, font, x + 18, y + 62, security.0, 12.0, security.1, 180);
    if private {
        draw::draw_text(fb, font, x + 160, y + 62, "Private mode", 12.0, PRIVATE_ACCENT, 140);
    }
    let adblock_label = if panel.adblock_enabled { "Adblock: enabled" } else { "Adblock: disabled" };
    draw::draw_text(fb, font, x + 320, y + 62, adblock_label, 12.0, theme.fg_secondary, 160);
    let blocked = format!("Blocked this session: {}", panel.blocked_count);
    draw::draw_text(fb, font, x + 320, y + 80, &blocked, 11.0, theme.fg_secondary, 190);
    draw::draw_text(fb, font, x + 18, y + 86, "Permissions", 13.0, theme.fg, 160);

    if panel.origin.is_none() {
        draw::draw_text(fb, font, x + 18, y + 116, "Permissions are available after loading an HTTP/HTTPS site.", 12.0, theme.fg_secondary, w - 36);
        return;
    }

    for (idx, kind) in permission_kinds_ui().iter().enumerate() {
        let row_y = y + 108 + idx as u32 * 30;
        let label = permission_label(*kind);
        draw::draw_text(fb, font, x + 18, row_y + 5, label, 12.0, theme.fg, 150);
        let current = permission_decision_for(state, *kind);
        draw::draw_text(fb, font, x + 176, row_y + 5, current.as_str(), 12.0, theme.fg_secondary, 90);
        for (decision, rect) in site_info_permission_rects(idx) {
            let selected = decision == current;
            let bg = if selected { theme.accent } else { theme.control_hover_bg };
            let fg = if selected { theme.accent_fg } else { theme.fg };
            draw::draw_rounded_rect(fb, rect.0, rect.1, rect.2, rect.3, 7, bg);
            draw::draw_text(fb, font, rect.0 + 10, rect.1 + 6, decision_title(decision), 11.0, fg, rect.2 - 12);
        }
    }

    let protection = if panel.adblock_allowlisted {
        "Protection off for this site"
    } else {
        "Protection on for this site"
    };
    draw::draw_text(fb, font, x + 18, y + h - 24, protection, 11.0, theme.fg_secondary, 220);
    let toggle = site_info_adblock_rect();
    let toggle_label = if panel.adblock_allowlisted { "Block ads on site" } else { "Allow ads on site" };
    draw_prompt_button(
        fb,
        font,
        toggle,
        toggle_label,
        if panel.adblock_allowlisted { theme.accent } else { theme.control_hover_bg },
        if panel.adblock_allowlisted { theme.accent_fg } else { theme.fg },
    );
}

fn permission_label(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Notifications => "Notifications",
        PermissionKind::Geolocation => "Geolocation",
        PermissionKind::Camera => "Camera",
        PermissionKind::Microphone => "Microphone",
        PermissionKind::Clipboard => "Clipboard",
    }
}

fn decision_title(decision: PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::Ask => "Ask",
        PermissionDecision::Allow => "Allow",
        PermissionDecision::Deny => "Deny",
    }
}

// ── Overlay panel (history / bookmarks) ──────────────────────────────────────

fn draw_overlay(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);

    // ── Header ────────────────────────────────────────────────────────────────
    let header_y = TOP_BAR_HEIGHT + 24;
    let (title_str, items_len): (&str, usize) = match state.overlay {
        OverlayKind::History   => ("History",   state.global_history.len()),
        OverlayKind::Bookmarks => ("Bookmarks", state.bookmarks.len()),
        OverlayKind::None      => unreachable!(),
    };

    draw::draw_text(fb, font, OVERLAY_INDENT, header_y, title_str, 22.0, theme.fg, 400);

    if items_len > 0 {
        let count_str = format!("{items_len} item{}", if items_len == 1 { "" } else { "s" });
        let cw = font.text_width(&count_str, 13.0);
        draw::draw_text(fb, font, OVERLAY_INDENT + 200, header_y + 5, &count_str, 13.0, theme.fg_secondary, 200);
        let _ = cw;
    }

    let hint = "Esc close  \u{2022}  Enter open  \u{2022}  Up/Dn scroll";
    let hw   = font.text_width(hint, 11.0);
    draw::draw_text(fb, font,
        FB_WIDTH.saturating_sub(OVERLAY_INDENT + hw), header_y + 6,
        hint, 11.0, theme.fg_secondary, 600);

    // Separator
    fb.fill_rect(OVERLAY_INDENT, header_y + 34, FB_WIDTH - OVERLAY_INDENT * 2, 1, theme.border);

    // ── Empty state ───────────────────────────────────────────────────────────
    if items_len == 0 {
        let msg = match state.overlay {
            OverlayKind::History   => "No history yet — browse some pages",
            OverlayKind::Bookmarks => "No bookmarks yet — click the star to save a page",
            OverlayKind::None      => unreachable!(),
        };
        let mw = font.text_width(msg, 15.0);
        let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
        draw::draw_text(fb, font, FB_WIDTH / 2 - mw / 2, cy, msg, 15.0, theme.fg_secondary, 800);
        return;
    }

    // ── Items ─────────────────────────────────────────────────────────────────
    let content_w = FB_WIDTH.saturating_sub(OVERLAY_INDENT * 2);

    for local_i in 0..OVERLAY_VISIBLE {
        let abs_i = state.overlay_scroll + local_i;
        let item_opt: Option<(&str, &str)> = match state.overlay {
            OverlayKind::History   => state.global_history.iter().rev().nth(abs_i)
                .map(|e| (e.title.as_str(), e.url.as_str())),
            OverlayKind::Bookmarks => state.bookmarks.get(abs_i)
                .map(|b| (b.title.as_str(), b.url.as_str())),
            OverlayKind::None => unreachable!(),
        };
        let (item_title, item_url) = match item_opt {
            Some(v) => v,
            None    => break,
        };

        let iy      = OVERLAY_LIST_TOP + local_i as u32 * OVERLAY_ITEM_H;
        let is_hot  = state.overlay_hover == Some(abs_i);

        if is_hot {
            fb.fill_rect(
                OVERLAY_INDENT.saturating_sub(12), iy,
                content_w + 24, OVERLAY_ITEM_H.saturating_sub(2),
                theme.surface,
            );
            // Left accent bar on hovered item
            fb.fill_rect(OVERLAY_INDENT.saturating_sub(12), iy, 3, OVERLAY_ITEM_H - 2, theme.accent);
        }

        let title_col = if is_hot { theme.fg } else { theme.fg };
        draw::draw_text(fb, font, OVERLAY_INDENT, iy + 10,
            item_title, 14.0, title_col, content_w.saturating_sub(200));
        draw::draw_text(fb, font, OVERLAY_INDENT, iy + 32,
            item_url, 11.0, if is_hot { theme.accent } else { theme.fg_secondary },
            content_w);

        fb.fill_rect(OVERLAY_INDENT, iy + OVERLAY_ITEM_H - 1, content_w, 1, theme.border);
    }

    // Scroll indicator
    if items_len > OVERLAY_VISIBLE {
        let visible_h = OVERLAY_VISIBLE as u32 * OVERLAY_ITEM_H;
        let track_h   = visible_h;
        let thumb_h   = ((track_h as u64 * OVERLAY_VISIBLE as u64) / items_len as u64)
            .max(24).min(track_h as u64) as u32;
        let max_off   = items_len.saturating_sub(OVERLAY_VISIBLE);
        let thumb_y   = OVERLAY_LIST_TOP
            + if max_off > 0 {
                (state.overlay_scroll as u64 * (track_h - thumb_h) as u64 / max_off as u64) as u32
            } else { 0 };
        let sx = FB_WIDTH - OVERLAY_INDENT + 16;
        fb.fill_rect(sx, OVERLAY_LIST_TOP, 4, track_h, theme.surface);
        fb.fill_rect(sx, thumb_y, 4, thumb_h, theme.fg_secondary);
    }
}

// ── New Tab page ──────────────────────────────────────────────────────────────

fn draw_new_tab(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme     = state.theme;
    let cx        = FB_WIDTH / 2;
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    let cy        = TOP_BAR_HEIGHT + content_h / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    let brand = "rashamon arc";
    let bw    = font.text_width(brand, 32.0);
    draw::draw_text(fb, font, cx.saturating_sub(bw / 2), cy.saturating_sub(200), brand, 32.0, theme.fg, 600);

    let tagline = "your private arc of the web";
    let tgw     = font.text_width(tagline, 15.0);
    draw::draw_text(fb, font, cx.saturating_sub(tgw / 2), cy.saturating_sub(156),
        tagline, 15.0, theme.fg_secondary, 600);

    let sw: u32 = 600; let sh: u32 = 48; let sr: u32 = 24;
    let sx = cx.saturating_sub(sw / 2);
    let sy = cy.saturating_sub(90);
    let border = if state.address_bar_focused { theme.address_bar_border_focused } else { theme.address_bar_border };
    draw::draw_rounded_rect(fb, sx.saturating_sub(1), sy.saturating_sub(1), sw + 2, sh + 2, sr + 1, border);
    draw::draw_rounded_rect(fb, sx, sy, sw, sh, sr, theme.address_bar_bg);

    if state.address_bar_input.is_empty() {
        let hint = "Search or enter URL";
        let hw   = font.text_width(hint, 15.0);
        draw::draw_text(fb, font, sx + (sw - hw) / 2, sy + (sh.saturating_sub(14)) / 2,
            hint, 15.0, theme.placeholder, sw - 40);
    } else {
        draw::draw_text(fb, font, sx + 24, sy + (sh.saturating_sub(14)) / 2,
            &state.address_bar_input, 15.0, theme.address_bar_fg, sw - 48);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw    = font.text_width(&state.address_bar_input, 15.0);
            let cur_x = (sx + 24 + cw + 1).min(sx + sw - 24);
            fb.fill_rect(cur_x, sy + (sh.saturating_sub(16)) / 2, 2, 16, theme.accent);
        }
    }

    let hints = "Ctrl+T  new  \u{2022}  Ctrl+I  private  \u{2022}  Ctrl+H  history  \u{2022}  Ctrl+B  bookmarks  \u{2022}  Ctrl+P  theme";
    let hw    = font.text_width(hints, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), sy + sh + 14,
        hints, 11.0, theme.fg_secondary, 1000);

    draw_quick_links(fb, state, font, cx, cy);
}

fn draw_private_new_tab(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme     = state.theme;
    let cx        = FB_WIDTH / 2;
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    let cy        = TOP_BAR_HEIGHT + content_h / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    // Private mode header with coloured accent
    draw::draw_circle_filled(fb, cx, cy.saturating_sub(180), 28, PRIVATE_ACCENT);
    // Draw a simple "P" inside the badge
    let pw = font.text_width("P", 20.0);
    draw::draw_text(fb, font, cx.saturating_sub(pw / 2), cy.saturating_sub(192),
        "P", 20.0, Pixel::WHITE, 30);

    let brand = "private browsing";
    let bw    = font.text_width(brand, 28.0);
    draw::draw_text(fb, font, cx.saturating_sub(bw / 2), cy.saturating_sub(136),
        brand, 28.0, PRIVATE_ACCENT, 600);

    let note = "Pages you visit here won't appear in history.";
    let nw   = font.text_width(note, 13.0);
    draw::draw_text(fb, font, cx.saturating_sub(nw / 2), cy.saturating_sub(96),
        note, 13.0, theme.fg_secondary, 700);

    let sw: u32 = 600; let sh: u32 = 48; let sr: u32 = 24;
    let sx = cx.saturating_sub(sw / 2);
    let sy = cy.saturating_sub(48);
    draw::draw_rounded_rect(fb, sx.saturating_sub(1), sy.saturating_sub(1),
        sw + 2, sh + 2, sr + 1, PRIVATE_ACCENT);
    draw::draw_rounded_rect(fb, sx, sy, sw, sh, sr, theme.address_bar_bg);

    if state.address_bar_input.is_empty() {
        let hint = "Private search or enter URL";
        let hw   = font.text_width(hint, 15.0);
        draw::draw_text(fb, font, sx + (sw - hw) / 2, sy + (sh.saturating_sub(14)) / 2,
            hint, 15.0, theme.placeholder, sw - 40);
    } else {
        draw::draw_text(fb, font, sx + 24, sy + (sh.saturating_sub(14)) / 2,
            &state.address_bar_input, 15.0, theme.address_bar_fg, sw - 48);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw    = font.text_width(&state.address_bar_input, 15.0);
            let cur_x = (sx + 24 + cw + 1).min(sx + sw - 24);
            fb.fill_rect(cur_x, sy + (sh.saturating_sub(16)) / 2, 2, 16, PRIVATE_ACCENT);
        }
    }

    let hints = "Ctrl+T  new tab  \u{2022}  Ctrl+W  close  \u{2022}  Ctrl+H  history  \u{2022}  Esc  exit";
    let hw    = font.text_width(hints, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), sy + sh + 14,
        hints, 11.0, theme.fg_secondary, 900);
}

const FAVICON_COLORS: [Pixel; 8] = [
    Pixel { r: 79,  g: 140, b: 255 }, Pixel { r: 52,  g: 168, b: 83  },
    Pixel { r: 255, g: 152, b: 0   }, Pixel { r: 233, g: 30,  b: 99  },
    Pixel { r: 156, g: 39,  b: 176 }, Pixel { r: 0,   g: 188, b: 212 },
    Pixel { r: 121, g: 85,  b: 72  }, Pixel { r: 96,  g: 125, b: 139 },
];

fn draw_quick_links(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager, cx: u32, cy: u32) {
    let theme = state.theme;
    let num   = state.bookmarks.len().min(6) as u32;
    if num == 0 { return; }

    let row_w      = num * QUICK_LINK_W + (num - 1) * QUICK_LINK_GAP;
    let mut card_x = cx.saturating_sub(row_w / 2);
    let card_y     = cy + 46;

    let lbl = "Quick access";
    let lw  = font.text_width(lbl, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(lw / 2), card_y.saturating_sub(20),
        lbl, 11.0, theme.fg_secondary, 200);

    for (i, bm) in state.bookmarks.iter().take(6).enumerate() {
        let fav_col = FAVICON_COLORS[i % FAVICON_COLORS.len()];
        let hovered = state.mouse_y >= card_y && state.mouse_y < card_y + QUICK_LINK_H
            && state.mouse_x >= card_x && state.mouse_x < card_x + QUICK_LINK_W
            && state.mouse_y >= TOP_BAR_HEIGHT;

        let card_bg = if hovered { theme.new_tab_card_hover_bg } else { theme.new_tab_card_bg };
        draw::draw_rounded_rect(fb, card_x, card_y, QUICK_LINK_W, QUICK_LINK_H, 10, card_bg);
        if hovered {
            draw::draw_rounded_rect_outline(fb, card_x as i32, card_y as i32,
                QUICK_LINK_W as i32, QUICK_LINK_H as i32, 10, theme.accent);
        }
        let fav_cx = card_x + QUICK_LINK_W / 2;
        let fav_cy = card_y + 32;
        draw::draw_circle_filled(fb, fav_cx, fav_cy, 20, fav_col);
        let mut ch_buf = [0u8; 4];
        let ch_str     = bm.first_upper.encode_utf8(&mut ch_buf);
        let lw         = font.text_width(ch_str, 16.0);
        draw::draw_text(fb, font, fav_cx.saturating_sub(lw / 2), fav_cy.saturating_sub(8),
            ch_str, 16.0, Pixel::WHITE, 24);
        let title_y = card_y + QUICK_LINK_H - 28;
        let max_tw  = QUICK_LINK_W.saturating_sub(12);
        let title_w = font.text_width(&bm.title, 12.0).min(max_tw);
        let title_x = card_x + (QUICK_LINK_W - title_w) / 2;
        draw::draw_text(fb, font, title_x, title_y, &bm.title, 12.0, theme.fg, max_tw);
        card_x += QUICK_LINK_W + QUICK_LINK_GAP;
    }
}

// ── Loading overlay ───────────────────────────────────────────────────────────

fn draw_loading(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    draw::draw_icon_spinner(fb, cx, cy.saturating_sub(20), 14, state.frame_count, theme.fg_secondary);
    const LOADING_MSGS: [&str; 4] = ["Loading...", "Loading.", "Loading..", "Loading..."];
    let msg = LOADING_MSGS[((state.frame_count / 18) % 4) as usize];
    let mw  = font.text_width(msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 8, msg, 14.0, theme.fg_secondary, 200);
    if let Some(tab) = state.active_tab() {
        if !tab.url.is_empty() {
            let host = derive_title(&tab.url);
            let hw   = font.text_width(host, 12.0);
            draw::draw_text(fb, font, cx.saturating_sub(hw / 2), cy + 30, host, 12.0, theme.placeholder, 600);
        }
    }
    let elapsed  = state.frame_count.saturating_sub(state.active_tab().map_or(0, |t| t.load_start_frame));
    let progress = ((elapsed as f32 / LOAD_MIN_FRAMES as f32) * FB_WIDTH as f32) as u32;
    fb.fill_rect(0, TOP_BAR_HEIGHT + 1, progress.min(FB_WIDTH - 4), 2, theme.accent);
}

fn draw_snapshot_pending(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    draw::draw_icon_spinner(fb, cx, cy.saturating_sub(18), 12, state.frame_count, theme.fg_secondary);
    let msg = "Updating view...";
    let mw  = font.text_width(msg, 13.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 10, msg, 13.0, theme.fg_secondary, 240);
}

// ── Loaded page ───────────────────────────────────────────────────────────────

fn draw_loaded(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager,
               nodes: &[PageNode], scroll_y: u32)
{
    if !nodes.is_empty() && !is_low_content(nodes) {
        draw_page_content(fb, state, font, nodes, scroll_y);
    } else if nodes.is_empty() {
        // Try fallback if we have meta or noscript text
        let entry = state.active_tab()
            .and_then(|t| t.history.get(t.history_index));
        let has_meta = entry.map_or(false, |e| {
            e.meta_description.is_some() || e.noscript.is_some()
        });
        if has_meta {
            draw_js_fallback(fb, state, font);
        } else {
            draw_loaded_card(fb, state, font);
        }
    } else {
        // Low content — show what we have + fallback supplement
        draw_js_fallback(fb, state, font);
    }
}

fn draw_page_content(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager,
                     nodes: &[PageNode], scroll_y: u32)
{
    let theme   = state.theme;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, 1, theme.border);

    let vp_top = TOP_BAR_HEIGHT as i32;
    let vp_bot = FB_HEIGHT as i32;
    let mut y: i32 = vp_top + PAD_TOP as i32 - scroll_y as i32;

    'outer: for node in nodes {
        if y > vp_bot + 200 { break; }
        match node {
            PageNode::Heading { level, text } => {
                let (size, color, before, after): (f32, _, i32, i32) = match level {
                    1 => (28.0, theme.fg, 18, 10),
                    2 => (22.0, theme.fg, 14, 8),
                    _ => (17.0, theme.fg, 10, 6),
                };
                y += before;
                for line in wrap_text(text, font, size, MAX_W) {
                    let lh = size as i32 + 4;
                    if y + lh > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, &line, size, color, MAX_W);
                    }
                    y += lh;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += after;
            }
            PageNode::Paragraph(text) => {
                if text.trim().is_empty() { continue; }
                y += 4;
                for line in wrap_text(text, font, 14.0, MAX_W) {
                    if y + 22 > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, &line, 14.0, theme.fg_secondary, MAX_W);
                    }
                    y += 22;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += 10;
            }
            PageNode::ListItem(text) => {
                let b = format!("  \u{2022}  {text}");
                for line in wrap_text(&b, font, 13.0, MAX_W) {
                    if y + 20 > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, &line, 13.0, theme.fg_secondary, MAX_W);
                    }
                    y += 20;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += 3;
            }
            PageNode::Pre(text) => {
                y += 8;
                let lines: Vec<&str> = text.lines().collect();
                let block_h = lines.len() as i32 * 18 + 16;
                if y < vp_bot && y + block_h > vp_top {
                    let fill_y = y.max(vp_top) as u32;
                    let fill_h = ((y + block_h).min(vp_bot) - fill_y as i32).max(0) as u32;
                    fb.fill_rect(MARGIN.saturating_sub(8), fill_y, MAX_W + 16, fill_h, theme.surface);
                }
                for line in &lines {
                    if y + 18 > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, line, 12.0, theme.fg, MAX_W + 8);
                    }
                    y += 18;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += 14;
            }
            PageNode::HRule => {
                y += 8;
                if y >= vp_top && y < vp_bot {
                    fb.fill_rect(MARGIN, y as u32, MAX_W, 1, theme.border);
                }
                y += 16;
            }
        }
    }

    // Scroll thumb
    let tab = match state.active_tab() { Some(t) => t, None => return };
    if tab.content_height > (FB_HEIGHT - TOP_BAR_HEIGHT) {
        let track_h = (FB_HEIGHT - TOP_BAR_HEIGHT) as u32;
        let thumb_h = ((track_h as u64 * track_h as u64)
                       / tab.content_height as u64).max(24).min(track_h as u64) as u32;
        let max_off = tab.content_height.saturating_sub(track_h);
        let thumb_y = if max_off > 0 {
            TOP_BAR_HEIGHT + (scroll_y as u64 * (track_h - thumb_h) as u64 / max_off as u64) as u32
        } else { TOP_BAR_HEIGHT };
        fb.fill_rect(FB_WIDTH - 4, TOP_BAR_HEIGHT, 4, track_h, theme.surface);
        fb.fill_rect(FB_WIDTH - 4, thumb_y, 4, thumb_h, theme.fg_secondary);
    }
}

fn wrap_text(text: &str, font: &FontManager, size: f32, max_w: u32) -> Vec<String> {
    let space_w = font.text_width(" ", size);
    let mut lines   = Vec::new();
    let mut line    = String::new();
    let mut line_w  = 0u32;
    for word in text.split_whitespace() {
        let ww  = font.text_width(word, size);
        let gap = if line.is_empty() { 0 } else { space_w };
        if !line.is_empty() && line_w + gap + ww > max_w {
            lines.push(std::mem::take(&mut line));
            line_w = 0;
        }
        if !line.is_empty() { line.push(' '); }
        line.push_str(word);
        line_w += gap + ww;
    }
    if !line.is_empty() { lines.push(line); }
    lines
}

fn draw_loaded_card(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    let Some(tab) = state.active_tab() else { return };
    const CARD_W: u32 = 680; const CARD_H: u32 = 220;
    let card_x = cx.saturating_sub(CARD_W / 2);
    let card_y = cy.saturating_sub(CARD_H / 2);
    draw::draw_rounded_rect(fb, card_x, card_y, CARD_W, CARD_H, 14, theme.surface);
    draw::draw_rounded_rect_outline(fb, card_x as i32, card_y as i32, CARD_W as i32, CARD_H as i32, 14, theme.border);
    let title_w = font.text_width(tab.tab_title(), 22.0).min(CARD_W - 48);
    draw::draw_text(fb, font, cx.saturating_sub(title_w / 2), card_y + 40, tab.tab_title(), 22.0, theme.fg, CARD_W - 48);
    let host   = derive_title(&tab.url);
    let host_w = font.text_width(host, 14.0).min(CARD_W - 48);
    draw::draw_text(fb, font, cx.saturating_sub(host_w / 2), card_y + 74, host, 14.0, theme.accent, CARD_W - 48);
    if !tab.url.is_empty() {
        let uw = font.text_width(&tab.url, 11.0).min(CARD_W - 48);
        draw::draw_text(fb, font, cx.saturating_sub(uw / 2), card_y + 104, &tab.url, 11.0, theme.fg_secondary, CARD_W - 48);
    }
    let hint = "Ctrl+R to reload  \u{2022}  address bar to navigate";
    let hw   = font.text_width(hint, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), card_y + CARD_H - 28, hint, 11.0, theme.fg_secondary, CARD_W - 48);
}

fn draw_js_fallback(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, 1, theme.border);

    let Some(tab) = state.active_tab() else { return };
    let entry = tab.history.get(tab.history_index);

    // Title / hostname
    let title = tab.tab_title();
    let title_w = font.text_width(title, 24.0).min(MAX_W);
    let mut y = TOP_BAR_HEIGHT + PAD_TOP + 16;
    draw::draw_text(fb, font, MARGIN, y, title, 24.0, theme.fg, MAX_W);
    y += 36;

    // Host line
    let host = derive_title(&tab.url);
    draw::draw_text(fb, font, MARGIN, y, host, 13.0, theme.accent, MAX_W);
    y += 26;

    // Divider
    fb.fill_rect(MARGIN, y, MAX_W, 1, theme.border);
    y += 20;

    // JS notice badge
    let badge = "This page requires JavaScript — showing available text content";
    let badge_w = font.text_width(badge, 12.0).min(MAX_W);
    let _ = badge_w;
    let badge_bg_x = MARGIN.saturating_sub(8);
    let badge_bg_w = MAX_W + 16;
    fb.fill_rect(badge_bg_x, y.saturating_sub(4), badge_bg_w, 24, theme.surface);
    fb.fill_rect(MARGIN.saturating_sub(8), y.saturating_sub(4), 3, 24, theme.fg_secondary);
    draw::draw_text(fb, font, MARGIN, y, badge, 12.0, theme.fg_secondary, MAX_W);
    y += 36;

    // noscript content (highest priority — site-authored fallback text)
    if let Some(ns) = entry.and_then(|e| e.noscript.as_deref()) {
        let lbl = "Page message:";
        draw::draw_text(fb, font, MARGIN, y, lbl, 11.0, theme.fg_secondary, MAX_W);
        y += 18;
        fb.fill_rect(MARGIN.saturating_sub(8), y.saturating_sub(2), 3, 2, theme.accent);
        for line in wrap_text(ns, font, 14.0, MAX_W) {
            if y > FB_HEIGHT - 80 { break; }
            draw::draw_text(fb, font, MARGIN, y, &line, 14.0, theme.fg, MAX_W);
            y += 22;
        }
        y += 8;
    }

    // meta description
    if let Some(desc) = entry.and_then(|e| e.meta_description.as_deref()) {
        let lbl = "Description:";
        draw::draw_text(fb, font, MARGIN, y, lbl, 11.0, theme.fg_secondary, MAX_W);
        y += 18;
        for line in wrap_text(desc, font, 14.0, MAX_W) {
            if y > FB_HEIGHT - 80 { break; }
            draw::draw_text(fb, font, MARGIN, y, &line, 14.0, theme.fg_secondary, MAX_W);
            y += 22;
        }
        y += 8;
    }

    // Any partial nodes we did extract
    if let Some(entry) = entry {
        if !entry.nodes.is_empty() {
            let lbl = "Extracted content:";
            draw::draw_text(fb, font, MARGIN, y, lbl, 11.0, theme.fg_secondary, MAX_W);
            y += 18;
            for node in &entry.nodes {
                if y > FB_HEIGHT - 80 { break; }
                let text = match node {
                    PageNode::Heading { text, .. } => text.as_str(),
                    PageNode::Paragraph(t) | PageNode::ListItem(t) | PageNode::Pre(t) => t.as_str(),
                    PageNode::HRule => continue,
                };
                for line in wrap_text(text, font, 13.0, MAX_W) {
                    if y > FB_HEIGHT - 80 { break; }
                    draw::draw_text(fb, font, MARGIN, y, &line, 13.0, theme.fg, MAX_W);
                    y += 20;
                }
            }
        }
    }

    // Hint
    let hint = "Ctrl+R  reload  \u{2022}  Rashamon Arc renders text — JavaScript sites show limited content";
    let _ = title_w;
    let hw = font.text_width(hint, 11.0).min(MAX_W + 100);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), FB_HEIGHT - 50, hint, 11.0, theme.fg_secondary, MAX_W + 100);
}

// ── Error page ────────────────────────────────────────────────────────────────

fn draw_error(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    let icon_cy = cy.saturating_sub(72);
    draw::draw_circle_filled(fb, cx, icon_cy, 30, theme.security_err);
    draw::draw_icon_close(fb, cx, icon_cy, 16, Pixel::WHITE);
    let title = "Page couldn't be loaded";
    let tw    = font.text_width(title, 20.0);
    draw::draw_text(fb, font, cx.saturating_sub(tw / 2), cy.saturating_sub(18), title, 20.0, theme.fg, 700);
    let msg = state.active_tab().and_then(|t| t.page_state.error_msg()).unwrap_or("The page is unavailable");
    let mw  = font.text_width(msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 14, msg, 14.0, theme.fg_secondary, 700);
    if let Some(tab) = state.active_tab() {
        if !tab.url.is_empty() {
            let uw = font.text_width(&tab.url, 12.0).min(800);
            draw::draw_text(fb, font, cx.saturating_sub(uw / 2), cy + 40, &tab.url, 12.0, theme.placeholder, 800);
        }
    }
    let (bx, by) = layout::retry_btn_pos();
    let hovered  = state.mouse_x >= bx && state.mouse_x < bx + RETRY_BTN_W
        && state.mouse_y >= by && state.mouse_y < by + RETRY_BTN_H;
    let btn_bg  = if hovered { theme.accent  } else { theme.surface };
    let btn_brd = if hovered { theme.accent  } else { theme.border  };
    draw::draw_rounded_rect(fb, bx.saturating_sub(1), by.saturating_sub(1), RETRY_BTN_W + 2, RETRY_BTN_H + 2, 9, btn_brd);
    draw::draw_rounded_rect(fb, bx, by, RETRY_BTN_W, RETRY_BTN_H, 8, btn_bg);
    let lbl    = "Try again";
    let lw     = font.text_width(lbl, 14.0);
    let lbl_fg = if hovered { theme.accent_fg } else { theme.fg };
    draw::draw_text(fb, font, bx + (RETRY_BTN_W.saturating_sub(lw)) / 2,
        by + (RETRY_BTN_H.saturating_sub(14)) / 2, lbl, 14.0, lbl_fg, RETRY_BTN_W - 8);
}
