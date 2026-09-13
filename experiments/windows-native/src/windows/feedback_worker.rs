use captures_feedback::{DEFAULT_FEEDBACK_URL, FeedbackClient, FeedbackContext, FeedbackDraft};
use std::{
    sync::mpsc::{Receiver, Sender, channel},
    thread,
};

pub struct Event {
    pub request: u64,
    pub result: Result<(), String>,
}

struct Command {
    request: u64,
    draft: FeedbackDraft,
}

pub struct FeedbackWorker {
    sender: Sender<Command>,
    receiver: Receiver<Event>,
}

impl FeedbackWorker {
    pub fn new() -> Self {
        let (sender, commands) = channel::<Command>();
        let (events, receiver) = channel();
        thread::spawn(move || {
            let endpoint = std::env::var("CAPTURES_FEEDBACK_URL")
                .unwrap_or_else(|_| DEFAULT_FEEDBACK_URL.into());
            let client = FeedbackClient::new(&endpoint);
            for command in commands {
                let result = client.as_ref().map_err(Clone::clone).and_then(|client| {
                    client.submit(
                        command.draft,
                        FeedbackContext {
                            app_version: env!("CARGO_PKG_VERSION").into(),
                            os: "windows".into(),
                            os_version: "Windows".into(),
                            arch: std::env::consts::ARCH.into(),
                        },
                    )
                });
                if events
                    .send(Event {
                        request: command.request,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { sender, receiver }
    }

    pub fn submit(&self, request: u64, draft: FeedbackDraft) -> Result<(), String> {
        self.sender
            .send(Command { request, draft })
            .map_err(|_| "Feedback worker is unavailable.".into())
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }
}
