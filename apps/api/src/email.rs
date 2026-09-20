use async_trait::async_trait;
use aws_sdk_sesv2::{
    Client,
    types::{Body, Content, Destination, EmailContent, Message},
};

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send_code(&self, recipient: &str, code: &str) -> Result<(), ()>;
}

pub struct SesMailer {
    client: Client,
    from: String,
    configuration_set: Option<String>,
}

impl SesMailer {
    pub async fn new(from: String, configuration_set: Option<String>) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self {
            client: Client::new(&config),
            from,
            configuration_set,
        }
    }
}

#[async_trait]
impl Mailer for SesMailer {
    async fn send_code(&self, recipient: &str, code: &str) -> Result<(), ()> {
        let content = |data: String| {
            Content::builder()
                .data(data)
                .charset("UTF-8")
                .build()
                .map_err(|_| ())
        };
        let subject = content("Your Captures sign-in code".into())?;
        let text = content(format!(
            "Your Captures sign-in code is {code}. It expires in 10 minutes.\n\nIf you did not request this code, you can ignore this email."
        ))?;
        let html = content(format!(
            "<p>Your Captures sign-in code is:</p><p><strong style=\"font-size:32px;letter-spacing:5px\">{code}</strong></p><p>It expires in 10 minutes.</p>"
        ))?;
        let request = self
            .client
            .send_email()
            .from_email_address(&self.from)
            .destination(Destination::builder().to_addresses(recipient).build())
            .content(
                EmailContent::builder()
                    .simple(
                        Message::builder()
                            .subject(subject)
                            .body(Body::builder().text(text).html(html).build())
                            .build(),
                    )
                    .build(),
            );
        let request = if let Some(value) = &self.configuration_set {
            request.configuration_set_name(value)
        } else {
            request
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), request.send())
            .await
            .map_err(|_| ())?
            .map(|_| ())
            .map_err(|_| ())
    }
}
