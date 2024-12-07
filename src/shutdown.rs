use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

pub struct ShutdownHandle {
    cancel_signal: CancellationToken,
    finish_handle: UnboundedReceiver<()>,
}

pub struct FinishSignal {
    sender: UnboundedSender<()>,
}

impl ShutdownHandle {
    pub fn new() -> (Self, FinishSignal) {
        let cancel_signal = CancellationToken::new();
        let (finish_sender, finish_receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                cancel_signal,
                finish_handle: finish_receiver,
            },
            FinishSignal {
                sender: finish_sender,
            },
        )
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel_signal.clone()
    }

    pub async fn shutdown(mut self) {
        self.cancel_signal.cancel();
        self.finish_handle.recv().await;
    }
}

impl FinishSignal {
    pub fn finish(&self) {
        self.sender.send(()).unwrap();
    }
}
