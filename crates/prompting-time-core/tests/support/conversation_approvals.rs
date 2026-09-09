use prompting_time_core::app::PromptingTime;
use prompting_time_core::domain::ConversationId;
use prompting_time_core::store::ApprovalSummary;

/// Live gates must inspect each canonical conversation; root reads intentionally
/// exclude child controls. Fail before returning any actions if a fixture grows
/// beyond its declared bounds.
pub async fn load_subtree_approvals(
    app: &PromptingTime,
    root: ConversationId,
    pending: bool,
) -> Result<Vec<ApprovalSummary>, Box<dyn std::error::Error>> {
    const MAX_CONVERSATIONS: usize = 80;
    const MAX_APPROVALS: usize = 200;
    let mut conversations = vec![root];
    let mut index = 0;
    while index < conversations.len() {
        let mut cursor = None;
        loop {
            let page = app
                .list_child_conversation_overviews(conversations[index], cursor, 20)
                .await?;
            if conversations.len() + page.items.len() > MAX_CONVERSATIONS {
                return Err("approval fixture exceeded 80 conversation scopes".into());
            }
            conversations.extend(page.items.into_iter().map(|item| item.conversation.id));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        index += 1;
    }

    let mut approvals = Vec::new();
    for conversation in conversations {
        let mut cursor = None;
        loop {
            let page = app
                .load_approvals(conversation, cursor, pending, 20)
                .await?;
            if approvals.len() + page.items.len() > MAX_APPROVALS {
                return Err("approval fixture exceeded 200 approval records".into());
            }
            approvals.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok(approvals)
}
