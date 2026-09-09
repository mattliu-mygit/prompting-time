use super::*;

/// Internal execution ownership. Native identifiers do not establish independent
/// session control or transfer the run owner's workspace and approval fences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationBinding {
    Managed,
    Observed {
        owner_conversation_id: ConversationId,
        run_id: RunId,
        agent_id: AgentId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationPath<T> {
    pub items: Vec<T>,
    pub truncated: bool,
    pub owner_conversation_id: ConversationId,
}

const MAX_CONVERSATION_PATH_ITEMS: i64 = 64;

#[derive(Deserialize, Serialize)]
struct ChildCursor {
    parent_id: ConversationId,
    created_at: i64,
    id: ConversationId,
}

#[derive(FromRow)]
struct ChildConversationRow {
    #[sqlx(flatten)]
    conversation: ConversationRow,
    created_at: i64,
}

impl Store {
    /// Returns the nearest contiguous ancestry window, including the selected node.
    /// Execution owner provenance is independent of whether the root fits in this window.
    pub async fn load_conversation_path(
        &self,
        id: ConversationId,
    ) -> Result<ConversationPath<Conversation>, StoreError> {
        let owner_conversation_id = match self.conversation_binding(id).await? {
            ConversationBinding::Managed => id,
            ConversationBinding::Observed {
                owner_conversation_id,
                ..
            } => owner_conversation_id,
        };
        let mut transaction = self.pool.begin().await?;
        require_visible_ancestry(&mut transaction, id).await?;
        let mut rows = sqlx::query_as::<_, ConversationRow>(
            "WITH RECURSIVE ancestry(id, parent_id, depth) AS ( \
                 SELECT id, parent_id, 0 FROM conversations WHERE id = ? \
                 UNION ALL \
                 SELECT parent.id, parent.parent_id, ancestry.depth + 1 \
                 FROM conversations AS parent JOIN ancestry ON parent.id = ancestry.parent_id \
                 WHERE ancestry.depth < ? \
             ) SELECT conversations.id, conversations.parent_id, substr(title, 1, 256) AS title, \
                      workspace_id, status, updated_at \
               FROM ancestry JOIN conversations ON conversations.id = ancestry.id ORDER BY ancestry.depth",
        ).bind(id.to_string()).bind(MAX_CONVERSATION_PATH_ITEMS)
            .fetch_all(&mut *transaction).await?;
        transaction.commit().await?;
        let truncated = rows.len() > MAX_CONVERSATION_PATH_ITEMS as usize;
        rows.truncate(MAX_CONVERSATION_PATH_ITEMS as usize);
        let mut items = rows
            .into_iter()
            .map(|row| row.into_record().map(|record| record.conversation))
            .collect::<Result<Vec<_>, _>>()?;
        items.reverse();
        Ok(ConversationPath {
            items,
            truncated,
            owner_conversation_id,
        })
    }

    pub async fn conversation_binding(
        &self,
        id: ConversationId,
    ) -> Result<ConversationBinding, StoreError> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT parent_id, observed_agent_id FROM conversations WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        let (parent_id, agent_id) = row.ok_or_else(|| StoreError::NotFound {
            entity: "conversation",
            id: id.to_string(),
        })?;
        match (parent_id, agent_id) {
            (None, None) => Ok(ConversationBinding::Managed),
            (Some(parent_id), Some(agent_id)) => {
                let binding: Option<(String, String)> = sqlx::query_as(
                    "SELECT run.conversation_id, run.id FROM agent_nodes AS agent \
                     JOIN provider_runs AS run ON run.id = agent.run_id \
                     JOIN conversations AS owner ON owner.id = run.conversation_id AND owner.parent_id IS NULL \
                     JOIN agent_nodes AS parent ON parent.id = agent.parent_id AND parent.run_id = agent.run_id \
                     JOIN conversations AS parent_conversation ON parent_conversation.id = ? \
                     WHERE agent.id = ? AND agent.provider = run.provider \
                       AND ((parent.parent_id IS NULL AND parent_conversation.id = run.conversation_id) \
                            OR parent_conversation.observed_agent_id = parent.id)",
                ).bind(parent_id).bind(&agent_id).fetch_optional(&self.pool).await?;
                let (owner, run) = binding.ok_or_else(invalid_binding)?;
                Ok(ConversationBinding::Observed {
                    owner_conversation_id: parse_uuid("conversation owner", &owner)?.into(),
                    run_id: parse_uuid("run", &run)?.into(),
                    agent_id: parse_uuid("agent", &agent_id)?.into(),
                })
            }
            _ => Err(invalid_binding()),
        }
    }

    /// Children retain newest-first creation order across lifecycle updates and
    /// later parent runs. The cursor is scoped to this parent, including its ID tie-break.
    pub async fn list_child_conversations(
        &self,
        parent_id: ConversationId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Page<Conversation>, StoreError> {
        validate_page_limit(limit)?;
        let cursor: Option<ChildCursor> = cursor
            .map(|value| {
                let bytes = URL_SAFE_NO_PAD
                    .decode(value)
                    .map_err(|_| StoreError::InvalidCursor)?;
                serde_json::from_slice(&bytes).map_err(|_| StoreError::InvalidCursor)
            })
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.parent_id != parent_id)
        {
            return Err(StoreError::InvalidCursor);
        }
        // One read transaction keeps ancestor visibility and this page consistent
        // with a concurrently archived root. UNION also terminates on corrupt cycles.
        let mut transaction = self.pool.begin().await?;
        require_visible_ancestry(&mut transaction, parent_id).await?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id, parent_id, substr(title, 1, 256) AS title, workspace_id, status, \
             updated_at, created_at FROM conversations WHERE status = 'active' AND parent_id = ",
        );
        query.push_bind(parent_id.to_string());
        if let Some(cursor) = cursor {
            query
                .push(" AND (created_at, id) < (")
                .push_bind(cursor.created_at)
                .push(", ")
                .push_bind(cursor.id.to_string())
                .push(")");
        }
        query
            .push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(i64::from(limit) + 1);
        let mut rows = query
            .build_query_as::<ChildConversationRow>()
            .fetch_all(&mut *transaction)
            .await?;
        transaction.commit().await?;
        let has_more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let next_cursor = if has_more {
            let last = rows.last().expect("a page with more rows is non-empty");
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&ChildCursor {
                        parent_id,
                        created_at: last.created_at,
                        id: parse_uuid("conversation", &last.conversation.id)?.into(),
                    })
                    .map_err(invalid_json("child cursor"))?,
                ),
            )
        } else {
            None
        };
        let items = rows
            .into_iter()
            .map(|row| {
                row.conversation
                    .into_record()
                    .map(|record| record.conversation)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page { items, next_cursor })
    }
}

async fn require_visible_ancestry(
    transaction: &mut Transaction<'_, Sqlite>,
    id: ConversationId,
) -> Result<(), StoreError> {
    let (ancestor_count, has_root, archived): (i64, bool, Option<String>) = sqlx::query_as(
        "WITH RECURSIVE ancestry(id, parent_id, status) AS ( \
             SELECT id, parent_id, status FROM conversations WHERE id = ? \
             UNION \
             SELECT parent.id, parent.parent_id, parent.status FROM conversations AS parent \
             JOIN ancestry ON parent.id = ancestry.parent_id \
         ) SELECT count(*), coalesce(max(parent_id IS NULL), 0), \
                  min(CASE WHEN status = 'archived' THEN id END) FROM ancestry",
    )
    .bind(id.to_string())
    .fetch_one(&mut **transaction)
    .await?;
    if ancestor_count == 0 {
        return Err(StoreError::NotFound {
            entity: "conversation",
            id: id.to_string(),
        });
    }
    if !has_root {
        return Err(invalid_binding());
    }
    if let Some(id) = archived {
        return Err(StoreError::ConversationArchived(
            parse_uuid("conversation", &id)?.into(),
        ));
    }
    Ok(())
}

fn invalid_binding() -> StoreError {
    StoreError::InvalidData {
        entity: "conversation binding",
        detail: "missing or conflicting execution ancestry".into(),
    }
}
