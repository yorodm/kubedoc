use crossterm::event::{Event as CrosstermEvent, KeyEvent};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
}

pub struct EventHandler {
    /// Keep the sender alive so the channel stays open; dropped by EventHandler::drop.
    _tx: mpsc::UnboundedSender<Event>,
    rx: mpsc::UnboundedReceiver<Event>,
    task: Option<JoinHandle<()>>,
}

impl EventHandler {
    pub fn new(_tick_rate: std::time::Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let task_tx = tx.clone();

        let task = tokio::spawn(async move {
            let mut reader = crossterm::event::EventStream::new();
            loop {
                let crossterm_event = reader.next().fuse();
                tokio::select! {
                    maybe_event = crossterm_event => {
                        match maybe_event {
                            Some(Ok(CrosstermEvent::Key(key))) => {
                                if key.kind == crossterm::event::KeyEventKind::Press {
                                    let _ = task_tx.send(Event::Key(key));
                                }
                            }
                            Some(Err(_)) => {}
                            _ => {}
                        }
                    }
                }
            }
        });

        Self {
            _tx: tx,
            rx,
            task: Some(task),
        }
    }

    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
