//! SecondBot's RabbitMQ commands (SecondBotEvents/Commands/RabbitMQ.cs): SendMessageToQ.
//!
//! As SecondBot's, the answers carry no arguments. Its "RabbitMQ service not available" never
//! comes: SecondBot answers it only when its service is missing, which it never is.

use super::Ctx;
use super::reply::{Answer, basic, failure};

pub async fn send_message_to_q(ctx: &Ctx, queue: &str, message: &str) -> Answer {
    const C: &str = "SendMessageToQ";
    let Some(rabbit) = &ctx.rabbit else {
        return failure(C, "RabbitMQ service not running", &[]);
    };
    if queue.is_empty() {
        return failure(C, "Qname is empty", &[]);
    }
    if message.is_empty() {
        return failure(C, "Message is empty", &[]);
    }
    match rabbit.send(queue, message).await {
        Ok(()) => basic(C, &format!("Message sent successfully to {queue}"), &[]),
        Err(e) => failure(C, &format!("Error sending message: {e}"), &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rabbit::{NOT_OPEN, Rabbit};
    use serde_json::Value;

    fn reply(answer: &str) -> (bool, String) {
        let v: Value = serde_json::from_str(answer).expect("json");
        assert_eq!(v["command"], "SendMessageToQ");
        assert_eq!(v["args"], serde_json::json!([]));
        (
            v["status"].as_bool().expect("status"),
            v["reply"].as_str().expect("reply").to_owned(),
        )
    }

    #[tokio::test]
    async fn off_without_rabbitmq() {
        let ctx = crate::commands::test_ctx();
        let (_, answer) =
            super::super::run(&ctx, "SendMessageToQ", &["q".into(), "m".into()]).await;
        assert_eq!(
            reply(&answer),
            (false, "RabbitMQ service not running".into())
        );
    }

    #[tokio::test]
    async fn sent_through_the_connection() {
        let (rabbit, mut outbox) = Rabbit::new();
        let mut ctx = crate::commands::test_ctx();
        ctx.rabbit = Some(rabbit);
        // a stand-in broker: takes the first, refuses the second as a closed connection does
        let stand_in = tokio::spawn(async move {
            let mut seen = Vec::new();
            for outcome in [Ok(()), Err(NOT_OPEN.to_owned())] {
                let p = outbox.recv().await.expect("a message");
                seen.push((p.queue, String::from_utf8(p.body).expect("text")));
                p.done.send(outcome).expect("the command waits");
            }
            seen
        });
        let args = ["exampleq".to_owned(), "Hello world".to_owned()];
        let (_, answer) = super::super::run(&ctx, "SendMessageToQ", &args).await;
        assert_eq!(
            reply(&answer),
            (true, "Message sent successfully to exampleq".into())
        );
        let (_, answer) = super::super::run(&ctx, "sendmessagetoq", &args).await;
        assert_eq!(
            reply(&answer),
            (
                false,
                "Error sending message: Connection is not open".into()
            )
        );
        assert_eq!(
            stand_in.await.expect("the stand-in"),
            vec![
                ("exampleq".to_owned(), "Hello world".to_owned()),
                ("exampleq".to_owned(), "Hello world".to_owned())
            ]
        );
        assert_eq!(
            reply(&send_message_to_q(&ctx, "", "m").await),
            (false, "Qname is empty".into())
        );
        assert_eq!(
            reply(&send_message_to_q(&ctx, "q", "").await),
            (false, "Message is empty".into())
        );
    }
}
