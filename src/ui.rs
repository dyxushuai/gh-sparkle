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
    step_labels: Vec<String>,
    steps_total: usize,
    current_step: usize,
    current_label: String,
    spinner_index: usize,
    last_tick: Instant,
    pending_log: Option<String>,
    shutdown: bool,
}

impl Ui {
    pub fn is_tty() -> bool {
        io::stdout().is_terminal()
    }

    pub fn start(step_labels: &[&str]) -> Result<Self, Box<dyn Error>> {
        let mut stdout = io::stdout();
        stdout.execute(Hide)?;
        let step_labels: Vec<String> = if step_labels.is_empty() {
            vec!["Starting".to_string()]
        } else {
            step_labels
                .iter()
                .map(|label| (*label).to_string())
                .collect()
        };
        let steps_total = step_labels.len().max(1);
        let current_step = 1;
        let label = format_step_line(current_step, steps_total, &step_labels[0]);

        Ok(Self {
            step_labels,
            steps_total,
            current_step,
            current_label: label,
            spinner_index: 0,
            last_tick: Instant::now(),
            pending_log: None,
            shutdown: false,
        })
    }

    pub fn shutdown(&mut self) -> Result<(), Box<dyn Error>> {
        if self.shutdown {
            return Ok(());
        }

        Self::clear_line()?;
        let mut stdout = io::stdout();
        stdout.execute(Show)?;
        stdout.flush()?;
        self.shutdown = true;
        Ok(())
    }

    pub fn set_step_status(&mut self, index: usize, status: StepStatus) {
        match status {
            StepStatus::Running => {
                self.current_step = index.saturating_add(1);
                let step_label = self
                    .step_labels
                    .get(index)
                    .map_or("Working", String::as_str);
                self.current_label =
                    format_step_line(self.current_step, self.steps_total, step_label);
            }
            StepStatus::Done => {
                if index.saturating_add(1) >= self.steps_total {
                    self.current_label = "Completed".to_string();
                }
            }
        }
    }

    pub fn set_error(&mut self) {
        self.current_label = "Failed".to_string();
    }

    pub fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        if message.is_empty() {
            return;
        }
        if self.pending_log.as_deref() == Some(message.as_str()) {
            return;
        }
        self.pending_log = Some(message);
    }

    pub fn tick(&mut self) {
        if self.last_tick.elapsed() >= Duration::from_millis(80) {
            self.spinner_index = (self.spinner_index + 1) % SPINNER_FRAMES.len();
            self.last_tick = Instant::now();
        }
    }

    pub fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(log) = self.pending_log.take() {
            Self::render_log_line(&log)?;
        }

        let spinner = SPINNER_FRAMES[self.spinner_index];
        let message = format!("{spinner} {label}", label = self.current_label);

        Self::render_line(&message)?;
        Ok(())
    }

    fn render_line(message: &str) -> Result<(), Box<dyn Error>> {
        let mut stdout = io::stdout();
        stdout.execute(MoveToColumn(0))?;
        stdout.execute(Clear(ClearType::CurrentLine))?;
        write!(stdout, "{message}")?;
        stdout.flush()?;
        Ok(())
    }

    fn render_log_line(message: &str) -> Result<(), Box<dyn Error>> {
        let mut stdout = io::stdout();
        stdout.execute(MoveToColumn(0))?;
        stdout.execute(Clear(ClearType::CurrentLine))?;
        writeln!(stdout, "ℹ {message}")?;
        stdout.flush()?;
        Ok(())
    }

    fn clear_line() -> Result<(), Box<dyn Error>> {
        let mut stdout = io::stdout();
        stdout.execute(MoveToColumn(0))?;
        stdout.execute(Clear(ClearType::CurrentLine))?;
        stdout.flush()?;
        Ok(())
    }
}

fn format_step_line(step: usize, total: usize, label: &str) -> String {
    format!("Step {step} of {total} — {label}")
}

impl Drop for Ui {
    fn drop(&mut self) {
        if self.shutdown {
            return;
        }

        let _ = Self::clear_line();
        let mut stdout = io::stdout();
        let _ = stdout.execute(Show);
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_step_line_matches_expected_output() {
        assert_eq!(
            format_step_line(3, 6, "Collect staged changes"),
            "Step 3 of 6 — Collect staged changes"
        );
    }
}
