use std::error::Error as StdError;
use std::io::{self, Error as IoError};
use std::path::Path;
use std::time::{Duration, Instant};

use vrac::Engine;

use super::ui::{content_width, draw_display_line, frame_lines, outline_height};
use super::{App, LauncherKind};

const SAMPLE_COUNT: usize = 100;
const RELOAD_COUNT: usize = 20;
const WIDTH: usize = 120;
const HEIGHT: usize = 40;
const FIRST_VIEW_BUDGET: Duration = Duration::from_millis(1_500);
const INTERACTIVE_BUDGET: Duration = Duration::from_micros(16_667);

pub fn run_reference_scenario(path: &Path) -> Result<Engine, Box<dyn StdError>> {
    if cfg!(debug_assertions) {
        return Err(IoError::other("the TUI performance scenario must run with --release").into());
    }

    let prepare_started = Instant::now();
    let engine = Engine::open(path)?;
    print_duration("database_prepare", prepare_started.elapsed());
    drop(engine);

    let first_view_started = Instant::now();
    let reopen_started = Instant::now();
    let engine = Engine::open(path)?;
    print_duration("database_reopen", reopen_started.elapsed());

    let app_started = Instant::now();
    let mut app = App::open_with_focus(engine, None)?;
    print_duration("tui_model_open", app_started.elapsed());
    record_budgeted(
        "first_view_model",
        first_view_started.elapsed(),
        FIRST_VIEW_BUDGET,
    )?;

    let first_frame_started = Instant::now();
    render_frame(&mut app)?;
    record_budgeted(
        "first_frame",
        first_frame_started.elapsed(),
        INTERACTIVE_BUDGET,
    )?;

    let frame_p95 = measure_p95(|| render_frame(&mut app))?;
    record_budgeted("frame_p95", frame_p95, INTERACTIVE_BUDGET)?;

    let mut direction = 1;
    let navigation_p95 = measure_p95(|| {
        app.move_selection(direction)?;
        direction *= -1;
        render_frame(&mut app)?;
        Ok(())
    })?;
    record_budgeted(
        "navigation_and_frame_p95",
        navigation_p95,
        INTERACTIVE_BUDGET,
    )?;

    let search_p95 = measure_p95(|| {
        app.start_launcher(LauncherKind::Search)?;
        for character in "decision 42".chars() {
            app.launcher
                .as_mut()
                .expect("search was just opened")
                .insert(character);
        }
        app.refresh_launcher()?;
        render_frame(&mut app)?;
        app.launcher = None;
        Ok(())
    })?;
    record_budgeted("search_and_frame_p95", search_p95, INTERACTIVE_BUDGET)?;

    let mut create_samples = Vec::with_capacity(SAMPLE_COUNT);
    for index in 0..SAMPLE_COUNT {
        let started = Instant::now();
        app.start_new_before();
        app.editor.as_mut().expect("creation opened an editor").text =
            format!("TUI creation performance probe {index}");
        if app.commit_editor()?.is_none() {
            return Err(IoError::other("the TUI creation probe was not persisted").into());
        }
        render_frame(&mut app)?;
        create_samples.push(started.elapsed());
    }
    record_budgeted(
        "persisted_create_and_frame_p95",
        percentile_95(&mut create_samples),
        INTERACTIVE_BUDGET,
    )?;

    let mut edit_samples = Vec::with_capacity(SAMPLE_COUNT);
    for index in 0..SAMPLE_COUNT {
        let started = Instant::now();
        app.start_edit();
        app.editor.as_mut().expect("edit opened an editor").text =
            format!("TUI edited performance probe {index}");
        if app.commit_editor()?.is_none() {
            return Err(IoError::other("the TUI edit probe was not persisted").into());
        }
        render_frame(&mut app)?;
        edit_samples.push(started.elapsed());
    }
    record_budgeted(
        "persisted_edit_and_frame_p95",
        percentile_95(&mut edit_samples),
        INTERACTIVE_BUDGET,
    )?;

    let mut reload_samples = Vec::with_capacity(RELOAD_COUNT);
    for _ in 0..RELOAD_COUNT {
        let started = Instant::now();
        app.reload_branch(None)?;
        render_frame(&mut app)?;
        reload_samples.push(started.elapsed());
    }
    record_budgeted(
        "reload_100_and_frame_p95",
        percentile_95(&mut reload_samples),
        INTERACTIVE_BUDGET,
    )?;

    Ok(app.engine)
}

fn render_frame(app: &mut App) -> Result<(), Box<dyn StdError>> {
    let content_width = content_width(WIDTH);
    let body_height = outline_height(HEIGHT, 0);
    app.viewport_width = content_width;
    let lines = frame_lines(app, content_width, body_height);
    let mut output = Vec::with_capacity(body_height * content_width);
    for line in lines.iter().skip(app.scroll).take(body_height) {
        draw_display_line(&mut output, line, content_width)?;
    }
    std::hint::black_box(output);
    Ok(())
}

fn measure_p95(
    mut operation: impl FnMut() -> Result<(), Box<dyn StdError>>,
) -> Result<Duration, Box<dyn StdError>> {
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let started = Instant::now();
        operation()?;
        samples.push(started.elapsed());
    }
    Ok(percentile_95(&mut samples))
}

fn percentile_95(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[(samples.len() * 95).div_ceil(100) - 1]
}

fn record_budgeted(name: &str, duration: Duration, budget: Duration) -> io::Result<()> {
    print_duration(name, duration);
    if duration > budget {
        return Err(IoError::other(format!(
            "{name} exceeded the {:.3} ms budget",
            budget.as_secs_f64() * 1_000.0
        )));
    }
    Ok(())
}

fn print_duration(name: &str, duration: Duration) {
    println!("{name}\t{:.3}\tms", duration.as_secs_f64() * 1_000.0);
}
