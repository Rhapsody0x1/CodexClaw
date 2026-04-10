use crate::{message::IncomingMessage, session::state::SessionSettings};

pub fn build_prompt(
    message: &IncomingMessage,
    settings: &SessionSettings,
    default_model: &str,
) -> String {
    let mut sections = Vec::new();
    sections.push(
        "You are CodexClaw running behind QQ official bot. Reply in Chinese unless the user asks otherwise.".to_string(),
    );
    sections.push(format!(
        "Current effective model: {}",
        settings.model_override.as_deref().unwrap_or(default_model)
    ));
    if settings.plan_mode {
        sections.push(
            "Plan mode is ON. Only produce an implementation plan. Do not claim to have changed files or executed actions.".to_string(),
        );
    }
    sections.push(
        "If you want QQ to send attachments, append one trailing fenced block named qqbot. Supported lines: `image path=REL_OR_ABS_PATH` and `file path=REL_OR_ABS_PATH name=DOWNLOAD_NAME`."
            .to_string(),
    );
    if let Some(quote) = &message.quote {
        sections.push(format!("Quoted message:\n{}", quote.text));
    }
    if !message.images.is_empty() {
        let list = message
            .images
            .iter()
            .map(|image| format!("- {}", image.local_path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "Images are attached separately and also available at:\n{}",
            list
        ));
    }
    if !message.files.is_empty() {
        let list = message
            .files
            .iter()
            .map(|file| {
                format!(
                    "- path: {}, filename: {}, content_type: {}",
                    file.local_path.display(),
                    file.filename.as_deref().unwrap_or("unknown"),
                    file.content_type.as_deref().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "User uploaded files. Read them from disk if needed:\n{}",
            list
        ));
    }
    let user_text = if message.text.trim().is_empty() {
        "(User sent no text, only attachments.)"
    } else {
        &message.text
    };
    sections.push(format!("User message:\n{}", user_text));
    sections.join("\n\n")
}
