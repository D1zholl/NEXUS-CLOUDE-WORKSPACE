//! Cooperative task control shared by the scheduler and file workers.

use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskCommand {
    Run,
    Pause,
    Cancel,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskControl {
    sender: watch::Sender<TaskCommand>,
}

impl TaskControl {
    pub fn new() -> Self {
        Self {
            sender: watch::channel(TaskCommand::Run).0,
        }
    }

    pub fn command(&self) -> TaskCommand {
        *self.sender.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<TaskCommand> {
        self.sender.subscribe()
    }

    pub fn pause(&self) {
        self.sender.send_replace(TaskCommand::Pause);
    }

    pub fn cancel(&self) {
        self.sender.send_replace(TaskCommand::Cancel);
    }
}

impl Default for TaskControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Sleeps, but wakes immediately on pause/cancel.
pub(crate) async fn sleep_or_stop(
    control: &TaskControl,
    duration: std::time::Duration,
) -> Option<TaskCommand> {
    if control.command() != TaskCommand::Run {
        return Some(control.command());
    }
    let mut receiver = control.subscribe();
    tokio::select! {
        _ = tokio::time::sleep(duration) => None,
        result = receiver.changed() => {
            if result.is_err() {
                Some(TaskCommand::Cancel)
            } else {
                let command = *receiver.borrow();
                if command == TaskCommand::Run { None } else { Some(command) }
            }
        }
    }
}
