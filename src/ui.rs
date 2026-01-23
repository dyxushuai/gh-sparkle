// Inline terminal UI rendering for gh-sparkle.

use crossterm::ExecutableCommand;
use crossterm::cursor::{Hide, MoveToColumn, Show};
use crossterm::terminal::{Clear, ClearType};
use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    Done,
}

pub struct Ui {
    steps_total: usize,
    current_step: usize,
    current_label: String,
    spinner_index: usize,
    last_tick: Instant,
    last_log: Option<String>,
}

impl Ui {
    pub fn is_tty() -> bool {
        io::stdout().is_terminal()
    }

    pub fn start(step_labels: Vec<&str>) -> Result<Self, Box<dyn Error>> {
        let mut stdout = io::stdout();
        stdout.execute(Hide)?;
        let label = step_labels
            .first()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Starting".to_string());

        Ok(Self {
            steps_total: step_labels.len().max(1),
            current_step: 1,
            current_label: label,
            spinner_index: 0,
            last_tick: Instant::now(),
            last_log: None,
        })
    }

    pub fn shutdown(mut self) -> Result<(), Box<dyn Error>> {
        self.clear_line()?;
        let mut stdout = io::stdout();
        stdout.execute(Show)?;
        stdout.flush()?;
        Ok(())
    }

    pub fn set_step_status(&mut self, index: usize, status: StepStatus) {
        if status == StepStatus::Running {
            self.current_step = index.saturating_add(1);
        }

        let label = match status {
            StepStatus::Running => format!("Step {} of {}", self.current_step, self.steps_total),
            StepStatus::Done => "Completed".to_string(),
        };

        self.current_label = label;
    }

    pub fn set_error(&mut self) {
        self.current_label = "Failed".to_string();
    }

    pub fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        if message.is_empty() {
            return;
        }
        self.last_log = Some(message);
    }

    pub fn tick(&mut self) {
        if self.last_tick.elapsed() >= Duration::from_millis(80) {
            self.spinner_index = (self.spinner_index + 1) % SPINNER_FRAMES.len();
            self.last_tick = Instant::now();
        }
    }

    pub fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        let spinner = SPINNER_FRAMES[self.spinner_index];
        let log = self.last_log.as_deref().unwrap_or("");
        let message = if log.is_empty() {
            format!("{spinner} {label}", label = self.current_label)
        } else {
            format!("{spinner} {label} — {log}", label = self.current_label)
        };

        self.render_line(&message)?;
        Ok(())
    }

    fn render_line(&mut self, message: &str) -> Result<(), Box<dyn Error>> {
        let mut stdout = io::stdout();
        stdout.execute(MoveToColumn(0))?;
        stdout.execute(Clear(ClearType::CurrentLine))?;
        write!(stdout, "{message}")?;
        stdout.flush()?;
        Ok(())
    }

    fn clear_line(&mut self) -> Result<(), Box<dyn Error>> {
        let mut stdout = io::stdout();
        stdout.execute(MoveToColumn(0))?;
        stdout.execute(Clear(ClearType::CurrentLine))?;
        stdout.flush()?;
        Ok(())
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        let _ = self.clear_line();
        let mut stdout = io::stdout();
        let _ = stdout.execute(Show);
        let _ = stdout.flush();
    }
}
