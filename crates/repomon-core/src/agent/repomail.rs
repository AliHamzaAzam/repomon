//! Parse fleet envelopes at the transcript boundary, preserving sender identity and prose.
use crate::model::{TranscriptItem, TranscriptMail};
use chrono::{DateTime, Utc};

pub fn split(text: &str, at: Option<DateTime<Utc>>) -> Vec<TranscriptItem> {
    let mut rows = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[REPOMAIL ") {
        let envelope = &rest[start..];
        let Some(header_end) = envelope.find(']') else {
            break;
        };
        let Some(end) = envelope[header_end + 1..]
            .find("[END REPOMAIL]")
            .map(|i| i + header_end + 1)
        else {
            break;
        };
        let mut id = None;
        let mut sender = None;
        let mut reply_to = None;
        for field in envelope[10..header_end].split_whitespace() {
            if let Some(v) = field.strip_prefix("id=") {
                id = Some(v);
            }
            if let Some(v) = field.strip_prefix("from=") {
                sender = Some(v);
            }
            if let Some(v) = field.strip_prefix("reply_to=").filter(|v| *v != "none") {
                reply_to = Some(v);
            }
        }
        let (Some(id), Some(sender)) = (
            id.filter(|s| !s.is_empty()),
            sender.filter(|s| !s.is_empty()),
        ) else {
            break;
        };
        if !rest[..start].trim().is_empty() {
            rows.push(TranscriptItem::new("user", rest[..start].trim(), at));
        }
        let body = envelope[header_end + 1..end].trim();
        let receipt = format!("[{id}]");
        let body = body.strip_suffix(&receipt).unwrap_or(body).trim();
        let mut item = TranscriptItem::new("mail", body, at);
        item.role = "user".into();
        item.mail = Some(TranscriptMail {
            id: id.into(),
            sender: sender.into(),
            reply_to: reply_to.map(str::to_string),
        });
        rows.push(item);
        rest = &envelope[end + "[END REPOMAIL]".len()..];
    }
    if !rest.trim().is_empty() {
        rows.push(TranscriptItem::new("user", rest, at));
    }
    rows
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_envelopes_are_structured_and_incomplete_text_is_preserved() {
        let rows = split(
            "hello\n[REPOMAIL id=abc from=lane-2/1 reply_to=parent] do this [abc] [END REPOMAIL]\nbye",
            None,
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].text, "do this");
        assert_eq!(rows[1].mail.as_ref().unwrap().sender, "lane-2/1");
        assert_eq!(
            rows[1].mail.as_ref().unwrap().reply_to.as_deref(),
            Some("parent")
        );
        assert_eq!(rows[2].text.trim(), "bye");
        assert_eq!(
            split("[REPOMAIL id=unfinished", None)[0].text,
            "[REPOMAIL id=unfinished"
        );
    }
}
