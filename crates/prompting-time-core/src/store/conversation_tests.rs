use super::*;

async fn logical_tree(
    store: &Store,
) -> (Conversation, ProviderRun, Vec<(ConversationId, AgentId)>) {
    let (conversation, run, root) = started_tree(store).await;
    observe(store, &run, &root, "root-native", &["first", "sibling"])
        .await
        .unwrap();
    observe(store, &run, &root, "first", &["grandchild"])
        .await
        .unwrap();
    let mut owners = vec![(conversation.id, root.id)];
    for native in ["first", "sibling", "grandchild"] {
        let (id, agent): (String, String) = sqlx::query_as(
            "SELECT conversations.id, agent_nodes.id FROM conversations JOIN agent_nodes \
             ON agent_nodes.id = conversations.observed_agent_id WHERE agent_nodes.run_id = ? \
             AND agent_nodes.provider_native_id = ?",
        )
        .bind(run.id.to_string())
        .bind(native)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        let agent = AgentId::from(parse_uuid("agent", &agent).unwrap());
        store
            .append_run_event(run.id, agent, ProviderEventRecord::started())
            .await
            .unwrap();
        owners.push((parse_uuid("conversation", &id).unwrap().into(), agent));
    }
    (conversation, run, owners)
}

#[tokio::test]
async fn unified_conversation_display_scopes_before_paging_and_preserves_raw_ownership() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, owners) = logical_tree(&store).await;
    let mut changes = store.subscribe_changes();
    for (id, agent) in &owners {
        for index in 0..5 {
            store
                .append_run_event(
                    run.id,
                    *agent,
                    ProviderEventRecord::message(format!("{id} message {index}")),
                )
                .await
                .unwrap();
            store
                .append_run_event(
                    run.id,
                    *agent,
                    ProviderEventRecord::unrecognized(format!("{id} telemetry {index}")),
                )
                .await
                .unwrap();
        }
    }
    for index in 0..90 {
        store
            .append_run_event(
                run.id,
                owners[0].1,
                ProviderEventRecord::message(format!("new root message {index}")),
            )
            .await
            .unwrap();
    }
    let mut change_count = 0;
    while let Ok(change) = changes.try_recv() {
        assert_eq!(
            change,
            StoreChange {
                conversation_id: conversation.id,
                run_id: Some(run.id)
            }
        );
        change_count += 1;
    }
    assert_eq!(
        change_count, 130,
        "one execution-owner invalidation per recorded event"
    );
    let temp = tempfile::tempdir().unwrap();
    let app = crate::app::PromptingTime::new(
        store.clone(),
        crate::router::Router::default(),
        crate::workspace::WorkspaceManager::new(temp.path()),
        vec![],
    )
    .unwrap();
    for (id, agent) in &owners {
        let expected: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM events WHERE agent_id = ? AND kind <> 'diagnostic' ORDER BY sequence",
        )
        .bind(agent.to_string())
        .fetch_all(&store.pool)
        .await
        .unwrap();
        let mut cursor = None;
        let mut events = Vec::new();
        loop {
            let page = app
                .load_timeline_snapshot(*id, cursor, 3)
                .await
                .unwrap()
                .events;
            assert!(
                page.items
                    .iter()
                    .all(|row| row.event.conversation_id == *id)
            );
            events.splice(
                0..0,
                page.items.into_iter().map(|row| row.event.id.to_string()),
            );
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(events, expected, "selected {id}");
        let diagnostics = app.load_diagnostics(*id, None, 80).await.unwrap();
        assert_eq!(diagnostics.items.len(), 5);
        assert!(
            diagnostics
                .items
                .iter()
                .all(|row| row.event.agent_id == *agent && row.event.conversation_id == *id)
        );
    }
    let child_id = owners[1].0;
    let expected: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM events WHERE agent_id = ? AND kind <> 'diagnostic' ORDER BY sequence",
    )
    .bind(owners[1].1.to_string())
    .fetch_all(&store.pool)
    .await
    .unwrap();
    let child_page = app
        .load_timeline_snapshot(child_id, None, 80)
        .await
        .unwrap();
    assert_eq!(
        child_page
            .events
            .items
            .iter()
            .map(|row| row.event.id.to_string())
            .collect::<Vec<_>>(),
        expected
    );
    let persisted_owners: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT conversation_id FROM events")
            .fetch_all(&store.pool)
            .await
            .unwrap();
    assert_eq!(persisted_owners, vec![conversation.id.to_string()]);
}

#[tokio::test]
async fn unified_conversation_approval_pages_keep_exact_execution_identity() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, owners) = logical_tree(&store).await;
    for (_, agent) in &owners {
        for index in 0..7 {
            for (status, resolution) in
                [("pending", None), ("denied", Some("{\"kind\":\"denied\"}"))]
            {
                sqlx::query("INSERT INTO approvals (id, conversation_id, run_id, agent_id, provider, \
                    provider_request_id, operation, scope, status, resolution_json, created_at, updated_at) \
                    VALUES (?, ?, ?, ?, 'codex', ?, 'review', 'turn', ?, ?, ?, ?)")
                    .bind(ApprovalId::new().to_string()).bind(conversation.id.to_string())
                    .bind(run.id.to_string()).bind(agent.to_string()).bind(format!("{agent}-{index}-{status}"))
                    .bind(status).bind(resolution).bind(index).bind(index)
                    .execute(&store.pool).await.unwrap();
            }
        }
    }
    for (id, agent) in owners {
        for pending in [true, false] {
            let mut cursor = None;
            let mut count = 0;
            loop {
                let page = store.load_approvals(id, cursor, pending, 3).await.unwrap();
                assert!(
                    page.items
                        .iter()
                        .all(|approval| approval.agent_id == agent && approval.run_id == run.id)
                );
                count += page.items.len();
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(count, 7);
        }
    }
}

#[tokio::test]
async fn unified_conversation_overviews_use_bound_child_status_after_later_parent_run() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, owners) = logical_tree(&store).await;
    sqlx::query("UPDATE agent_nodes SET summary = ? WHERE id = ?")
        .bind("child result ".repeat(500))
        .bind(owners[1].1.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
    store
        .fail_run_if_active(
            run.id,
            owners[0].1,
            ProviderErrorCategory::Protocol,
            MutationState::NoneObserved,
            DispatchCertainty::MayHaveDispatched,
        )
        .await
        .unwrap();
    let (next_run, _) = started_run(&store, conversation.id).await;
    let temp = tempfile::tempdir().unwrap();
    let app = crate::app::PromptingTime::new(
        store.clone(),
        crate::router::Router::default(),
        crate::workspace::WorkspaceManager::new(temp.path()),
        vec![],
    )
    .unwrap();
    let root = app
        .load_conversation_overview(conversation.id)
        .await
        .unwrap();
    assert!(root.has_children && root.capabilities.can_send && root.capabilities.can_archive);
    assert_eq!(root.run.unwrap().id, next_run.id);
    let root_history = app
        .load_timeline_snapshot(conversation.id, None, 80)
        .await
        .unwrap();
    assert!(
        root_history
            .events
            .items
            .iter()
            .any(|row| row.event.run_id == run.id)
    );
    assert!(
        root_history
            .events
            .items
            .iter()
            .any(|row| row.event.run_id == next_run.id)
    );
    let child = app.load_conversation_overview(owners[1].0).await.unwrap();
    assert_eq!(child.run.unwrap().id, run.id);
    assert_eq!(child.run.unwrap().status, RunStatus::Failed);
    assert!(child.has_children);
    assert_eq!(
        child.summary.unwrap().len(),
        MAX_AGENT_SUMMARY_PREVIEW_BYTES
    );
    assert!(
        !child.capabilities.can_send
            && !child.capabilities.can_interrupt
            && !child.capabilities.can_archive
            && !child.capabilities.can_route
    );
    assert_eq!(
        child.capabilities.unavailable_reason.as_deref(),
        Some(
            "This provider exposes recorded child activity, not an independently controllable chat."
        )
    );
    let siblings = app
        .list_child_conversation_overviews(conversation.id, None, 1)
        .await
        .unwrap();
    let next = app
        .list_child_conversation_overviews(conversation.id, siblings.next_cursor, 1)
        .await
        .unwrap();
    assert_eq!(siblings.items.len() + next.items.len(), 2);
    assert!(
        siblings
            .items
            .iter()
            .chain(next.items.iter())
            .all(|item| item.conversation.parent_id == Some(conversation.id))
    );
    let path = app.load_conversation_path(owners[3].0).await.unwrap();
    assert_eq!(
        path.items
            .iter()
            .map(|item| item.conversation.id)
            .collect::<Vec<_>>(),
        vec![conversation.id, owners[1].0, owners[3].0]
    );
    assert_eq!(path.owner_conversation_id, conversation.id);
    assert!(!path.truncated);
}

#[tokio::test]
async fn unified_conversation_rollup_includes_own_descendants_but_excludes_siblings() {
    let store = Store::open_in_memory().await.unwrap();
    let (_, run, owners) = logical_tree(&store).await;
    let temp = tempfile::tempdir().unwrap();
    let app = crate::app::PromptingTime::new(
        store.clone(),
        crate::router::Router::default(),
        crate::workspace::WorkspaceManager::new(temp.path()),
        vec![],
    )
    .unwrap();
    store
        .append_run_event(run.id, owners[3].1, ProviderEventRecord::waiting())
        .await
        .unwrap();
    let child = app.load_conversation_overview(owners[1].0).await.unwrap();
    assert_eq!(
        child.run.unwrap().status,
        RunStatus::Running,
        "own execution stays running"
    );
    assert_eq!(
        child.rollup_status,
        Some(crate::domain::RollupStatus::NeedsAttention)
    );
    assert_eq!(
        app.load_conversation_overview(owners[2].0)
            .await
            .unwrap()
            .rollup_status,
        Some(crate::domain::RollupStatus::Active)
    );
    store
        .append_run_event(run.id, owners[3].1, ProviderEventRecord::resumed())
        .await
        .unwrap();
    store
        .append_run_event(run.id, owners[2].1, ProviderEventRecord::waiting())
        .await
        .unwrap();
    assert_eq!(
        app.load_conversation_overview(owners[1].0)
            .await
            .unwrap()
            .rollup_status,
        Some(crate::domain::RollupStatus::Active),
        "a waiting sibling must not affect this branch"
    );
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "EXPLAIN QUERY PLAN WITH RECURSIVE subtree(id, status) AS ( \
             SELECT id, status FROM agent_nodes WHERE id = ? \
             UNION ALL SELECT child.id, child.status FROM subtree CROSS JOIN agent_nodes AS child \
             WHERE child.parent_id = subtree.id AND child.run_id = ? \
         ) SELECT MAX(CASE status WHEN 'waiting' THEN 5 ELSE 1 END) FROM subtree",
    )
    .bind(owners[1].1.to_string())
    .bind(run.id.to_string())
    .fetch_all(&store.pool)
    .await
    .unwrap();
    assert!(
        plan.iter().any(|row| row
            .3
            .contains("SEARCH child USING INDEX idx_agents_run_parent")),
        "{plan:?}"
    );
    assert!(
        !plan.iter().any(|row| row.3.contains("SCAN child")),
        "{plan:?}"
    );
}

#[tokio::test]
async fn unified_conversation_path_is_contiguous_bounded_and_rejects_archived_ancestry() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, root) = started_tree(&store).await;
    let mut native = "root-native".to_owned();
    for depth in 0..70 {
        let child = format!("native-{depth}");
        observe(&store, &run, &root, &native, &[&child])
            .await
            .unwrap();
        native = child;
    }
    let selected: String = sqlx::query_scalar(
        "SELECT conversations.id FROM conversations JOIN agent_nodes
        ON agent_nodes.id = conversations.observed_agent_id WHERE provider_native_id = ?",
    )
    .bind(native)
    .fetch_one(&store.pool)
    .await
    .unwrap();
    let selected = ConversationId::from(parse_uuid("conversation", &selected).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let app = crate::app::PromptingTime::new(
        store.clone(),
        crate::router::Router::default(),
        crate::workspace::WorkspaceManager::new(temp.path()),
        vec![],
    )
    .unwrap();
    let path = app.load_conversation_path(selected).await.unwrap();
    assert_eq!(path.items.len(), 64);
    assert!(path.truncated);
    assert_eq!(path.owner_conversation_id, conversation.id);
    assert_eq!(path.items.last().unwrap().conversation.id, selected);
    assert!(path.items.first().unwrap().conversation.parent_id.is_some());
    for pair in path.items.windows(2) {
        assert_eq!(
            pair[1].conversation.parent_id,
            Some(pair[0].conversation.id)
        );
    }
    store
        .fail_run_if_active(
            run.id,
            root.id,
            ProviderErrorCategory::Protocol,
            MutationState::NoneObserved,
            DispatchCertainty::MayHaveDispatched,
        )
        .await
        .unwrap();
    app.archive(conversation.id).await.unwrap();
    assert!(matches!(app.load_conversation_path(selected).await,
        Err(crate::app::AppError::Store(StoreError::ConversationArchived(id))) if id == conversation.id));
}

async fn started_tree(store: &Store) -> (Conversation, ProviderRun, AgentNode) {
    let conversation = store
        .create_conversation(NewConversation::projectless("owner"))
        .await
        .unwrap();
    let (run, root) = started_run(store, conversation.id).await;
    (conversation, run, root)
}

async fn started_run(store: &Store, id: ConversationId) -> (ProviderRun, AgentNode) {
    let (run, root) = store.create_run(id, ProviderId::Codex).await.unwrap();
    store
        .bind_native_session(run.id, "root-native")
        .await
        .unwrap();
    store
        .append_run_event(run.id, root.id, ProviderEventRecord::started())
        .await
        .unwrap();
    (run, root)
}

async fn observe(
    store: &Store,
    run: &ProviderRun,
    root: &AgentNode,
    parent: &str,
    children: &[&str],
) -> Result<TimelineEvent, StoreError> {
    store
        .append_run_event(
            run.id,
            root.id,
            ProviderEventRecord::child_agent(
                Uuid::now_v7().to_string(),
                parent,
                children.iter().map(|id| (*id).to_owned()).collect(),
                vec![],
                "spawn",
                "completed",
            ),
        )
        .await
}

#[tokio::test]
async fn unified_conversation_materializes_one_child_per_execution_and_preserves_workspace() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, root) = started_tree(&store).await;
    let workspace_id = Uuid::now_v7().to_string();
    let mut transaction = store.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO workspaces (id, conversation_id, execution_path, owned_worktree, created_at, updated_at) VALUES (?, ?, '/invented/workspace', 0, 1, 1)")
        .bind(&workspace_id).bind(conversation.id.to_string())
        .execute(&mut *transaction).await.unwrap();
    sqlx::query("UPDATE conversations SET workspace_id = ? WHERE id = ?")
        .bind(&workspace_id)
        .bind(conversation.id.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    observe(&store, &run, &root, "root-native", &["child"])
        .await
        .unwrap();
    observe(&store, &run, &root, "child", &["grandchild"])
        .await
        .unwrap();
    observe(&store, &run, &root, "root-native", &["child"])
        .await
        .unwrap();
    store
        .append_run_event(
            run.id,
            root.id,
            ProviderEventRecord::sub_agent(
                "child-start",
                "child",
                "/child",
                NativeSubAgentActivityKind::Started,
            ),
        )
        .await
        .unwrap();

    let children = store
        .list_child_conversations(conversation.id, None, 20)
        .await
        .unwrap();
    assert_eq!(children.items.len(), 1);
    let child = &children.items[0];
    assert_eq!(child.parent_id, Some(conversation.id));
    assert_ne!(child.id, conversation.id);
    assert_eq!(child.workspace_id, None);
    let grandchildren = store
        .list_child_conversations(child.id, None, 20)
        .await
        .unwrap();
    assert_eq!(grandchildren.items.len(), 1);
    assert_eq!(grandchildren.items[0].parent_id, Some(child.id));
    assert_eq!(
        store.conversation_binding(conversation.id).await.unwrap(),
        ConversationBinding::Managed
    );
    let agent_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_nodes WHERE run_id = ? AND provider_native_id = 'child'",
    )
    .bind(run.id.to_string())
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert_eq!(
        store.conversation_binding(child.id).await.unwrap(),
        ConversationBinding::Observed {
            owner_conversation_id: conversation.id,
            run_id: run.id,
            agent_id: parse_uuid("agent", &agent_id).unwrap().into(),
        }
    );
    assert_eq!(
        store
            .list_active_conversations(None, 20)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let workspaces: Vec<(String, String)> =
        sqlx::query_as("SELECT id, conversation_id FROM workspaces")
            .fetch_all(&store.pool)
            .await
            .unwrap();
    assert_eq!(
        workspaces,
        vec![(workspace_id, conversation.id.to_string())]
    );
    for query in [
        "SELECT count(*) FROM provider_sessions WHERE conversation_id = ?",
        "SELECT count(*) FROM conversation_settings WHERE conversation_id = ?",
        "SELECT count(*) FROM provider_runs WHERE conversation_id = ?",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .bind(child.id.to_string())
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "observed child must not acquire execution resources: {query}"
        );
    }
}

#[tokio::test]
async fn unified_conversation_observation_rollback_leaves_no_child() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, root) = started_tree(&store).await;
    observe(&store, &run, &root, "root-native", &["a", "b"])
        .await
        .unwrap();
    assert!(matches!(
        observe(&store, &run, &root, "a", &["stray", "b"]).await,
        Err(StoreError::NativeAgentIdentityConflict)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversations")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    let child = store
        .list_child_conversations(conversation.id, None, 20)
        .await
        .unwrap()
        .items
        .remove(0);
    assert!(
        store
            .list_child_conversations(child.id, None, 20)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_nodes WHERE provider_native_id = 'stray'")
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn unified_conversation_children_survive_new_runs_and_archive_hides_subtree() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, root) = started_tree(&store).await;
    observe(&store, &run, &root, "root-native", &["same-native"])
        .await
        .unwrap();
    observe(&store, &run, &root, "same-native", &["grandchild"])
        .await
        .unwrap();
    let old_child = store
        .list_child_conversations(conversation.id, None, 20)
        .await
        .unwrap()
        .items
        .remove(0);
    store
        .fail_run_if_active(
            run.id,
            root.id,
            ProviderErrorCategory::Protocol,
            MutationState::NoneObserved,
            DispatchCertainty::MayHaveDispatched,
        )
        .await
        .unwrap();
    let (next_run, next_root) = started_run(&store, conversation.id).await;
    observe(
        &store,
        &next_run,
        &next_root,
        "root-native",
        &["same-native"],
    )
    .await
    .unwrap();
    let children = store
        .list_child_conversations(conversation.id, None, 20)
        .await
        .unwrap();
    assert_eq!(children.items.len(), 2);
    assert!(children.items.iter().any(|item| item.id == old_child.id));
    assert!(
        matches!(store.conversation_binding(old_child.id).await.unwrap(),
        ConversationBinding::Observed { run_id, .. } if run_id == run.id)
    );
    store
        .fail_run_if_active(
            next_run.id,
            next_root.id,
            ProviderErrorCategory::Protocol,
            MutationState::NoneObserved,
            DispatchCertainty::MayHaveDispatched,
        )
        .await
        .unwrap();
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM agent_nodes ORDER BY id")
            .fetch_all(&store.pool)
            .await
            .unwrap();
    store.archive_conversation(conversation.id).await.unwrap();
    assert!(
        store
            .list_active_conversations(None, 20)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    for id in [conversation.id, old_child.id] {
        assert!(matches!(
            store.list_child_conversations(id, None, 20).await,
            Err(StoreError::ConversationArchived(_))
        ));
    }
    assert!(
        !store
            .load_conversation(old_child.id)
            .await
            .unwrap()
            .archived
    );
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM agent_nodes ORDER BY id")
            .fetch_all(&store.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn unified_conversation_child_pages_are_stable_and_scoped_to_visible_parent() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, root) = started_tree(&store).await;
    observe(
        &store,
        &run,
        &root,
        "root-native",
        &["a", "b", "c", "d", "e"],
    )
    .await
    .unwrap();
    // All siblings deliberately share a timestamp; the ID tie-breaker must cover every row.
    sqlx::query("UPDATE conversations SET created_at = 1 WHERE parent_id = ?")
        .bind(conversation.id.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
    let first = store
        .list_child_conversations(conversation.id, None, 2)
        .await
        .unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    let sibling = first.items[0].id;
    assert!(matches!(
        store
            .list_child_conversations(sibling, Some(cursor), 2)
            .await,
        Err(StoreError::InvalidCursor)
    ));
    let mut ids: Vec<_> = first.items.iter().map(|item| item.id).collect();
    let mut cursor = first.next_cursor;
    while let Some(next) = cursor {
        let page = store
            .list_child_conversations(conversation.id, Some(next), 2)
            .await
            .unwrap();
        ids.extend(page.items.into_iter().map(|item| item.id));
        cursor = page.next_cursor;
    }
    assert_eq!(ids.len(), 5);
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 5);
    assert!(
        ids.windows(2)
            .all(|pair| pair[0].to_string() > pair[1].to_string())
    );
    observe(&store, &run, &root, "root-native", &["newest"])
        .await
        .unwrap();
    let refreshed = store
        .list_child_conversations(conversation.id, None, 2)
        .await
        .unwrap();
    assert!(refreshed.items.iter().any(|item| !ids.contains(&item.id)));
    assert!(matches!(
        store
            .list_child_conversations(ConversationId::new(), None, 2)
            .await,
        Err(StoreError::NotFound { .. })
    ));
    assert!(matches!(
        store.conversation_binding(ConversationId::new()).await,
        Err(StoreError::NotFound { .. })
    ));
    assert!(matches!(
        store
            .list_child_conversations(conversation.id, None, 0)
            .await,
        Err(StoreError::InvalidPageLimit(0))
    ));
}

struct LegacyFixture {
    owner: ConversationId,
    root: AgentId,
    child: AgentId,
    grandchild: AgentId,
    run: RunId,
}

async fn legacy_fixture(pool: &SqlitePool) -> LegacyFixture {
    MIGRATOR.run_to(18, pool).await.unwrap();
    let fixture = LegacyFixture {
        owner: ConversationId::new(),
        root: AgentId::new(),
        child: AgentId::new(),
        grandchild: AgentId::new(),
        run: RunId::new(),
    };
    sqlx::query("INSERT INTO conversations (id, title, status, created_at, updated_at) VALUES (?, 'legacy owner', 'active', 1, 1)")
        .bind(fixture.owner.to_string()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO provider_runs (id, conversation_id, provider, native_session_id, status, mutation_state, created_at, updated_at) VALUES (?, ?, 'codex', 'legacy-root', 'completed', 'none_observed', 1, 4)")
        .bind(fixture.run.to_string()).bind(fixture.owner.to_string()).execute(pool).await.unwrap();
    for (agent, parent, native, depth) in [
        (fixture.root, None, "legacy-root", 0),
        (fixture.child, Some(fixture.root), "legacy-child", 1),
        (
            fixture.grandchild,
            Some(fixture.child),
            "legacy-grandchild",
            2,
        ),
    ] {
        sqlx::query("INSERT INTO agent_nodes (id, run_id, parent_id, provider, label, provider_native_id, status, depth, created_at, updated_at) VALUES (?, ?, ?, 'codex', ?, ?, 'completed', ?, 2, 4)")
            .bind(agent.to_string()).bind(fixture.run.to_string()).bind(parent.map(|id| id.to_string())).bind(native).bind(native).bind(depth).execute(pool).await.unwrap();
    }
    let mut transaction = pool.begin().await.unwrap();
    let workspace = Uuid::now_v7().to_string();
    sqlx::query("INSERT INTO workspaces (id, conversation_id, execution_path, owned_worktree, created_at, updated_at) VALUES (?, ?, '/invented/legacy', 0, 1, 1)")
        .bind(&workspace).bind(fixture.owner.to_string()).execute(&mut *transaction).await.unwrap();
    sqlx::query("UPDATE conversations SET workspace_id = ? WHERE id = ?")
        .bind(workspace)
        .bind(fixture.owner.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    sqlx::query("INSERT INTO approvals (id, conversation_id, run_id, agent_id, provider, provider_request_id, operation, scope, status, resolution_json, created_at, updated_at) VALUES (?, ?, ?, ?, 'codex', 'legacy-approval', 'write', 'file', 'approved', '{\"kind\":\"approved\"}', 3, 4)")
        .bind(ApprovalId::new().to_string()).bind(fixture.owner.to_string()).bind(fixture.run.to_string()).bind(fixture.child.to_string()).execute(pool).await.unwrap();
    let offset: i64 = sqlx::query_scalar("SELECT coalesce(max(sequence), 0) FROM events")
        .fetch_one(pool)
        .await
        .unwrap();
    for (sequence, agent) in [
        (17 + offset, fixture.root),
        (23 + offset, fixture.child),
        (40 + offset, fixture.grandchild),
    ] {
        sqlx::query("INSERT INTO events (sequence, id, conversation_id, run_id, agent_id, kind, role, content, created_at) VALUES (?, ?, ?, ?, ?, 'message', 'assistant', 'captured', 3)")
            .bind(sequence).bind(TimelineEventId::new().to_string()).bind(fixture.owner.to_string()).bind(fixture.run.to_string()).bind(agent.to_string()).execute(pool).await.unwrap();
    }
    fixture
}

async fn historical_snapshot(pool: &SqlitePool) -> Vec<String> {
    let mut snapshots = Vec::new();
    for query in [
        "SELECT json_group_array(json_array(id, conversation_id, status, agent_total_count)) FROM (SELECT * FROM provider_runs ORDER BY id)",
        "SELECT json_group_array(json_array(id, run_id, parent_id, status, provider_native_id, depth)) FROM (SELECT * FROM agent_nodes ORDER BY id)",
        "SELECT json_group_array(json_array(id, sequence, conversation_id, run_id, agent_id, content)) FROM (SELECT * FROM events ORDER BY sequence)",
        "SELECT json_group_array(json_array(id, conversation_id, run_id, agent_id, status, resolution_json)) FROM (SELECT * FROM approvals ORDER BY id)",
        "SELECT json_group_array(json_array(id, conversation_id, execution_path, owned_worktree)) FROM (SELECT * FROM workspaces ORDER BY id)",
    ] {
        snapshots.push(sqlx::query_scalar(query).fetch_one(pool).await.unwrap());
    }
    snapshots
}

#[tokio::test]
async fn unified_conversation_migration_preserves_history_and_reopen_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("migration.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    let fixture = legacy_fixture(&pool).await;
    let before = historical_snapshot(&pool).await;
    pool.close().await;
    let store = Store::open(&path).await.unwrap();
    assert_eq!(historical_snapshot(&store.pool).await, before);
    let children = store
        .list_child_conversations(fixture.owner, None, 20)
        .await
        .unwrap();
    assert_eq!(children.items.len(), 1);
    let child = children.items[0].clone();
    assert_eq!(child.parent_id, Some(fixture.owner));
    assert_ne!(child.id.as_uuid(), fixture.child.as_uuid());
    assert_eq!(
        store.conversation_binding(child.id).await.unwrap(),
        ConversationBinding::Observed {
            owner_conversation_id: fixture.owner,
            run_id: fixture.run,
            agent_id: fixture.child,
        }
    );
    assert_eq!(
        store
            .list_child_conversations(child.id, None, 20)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    store.close().await;
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .list_child_conversations(fixture.owner, None, 20)
            .await
            .unwrap()
            .items,
        vec![child]
    );
    assert_eq!(historical_snapshot(&reopened.pool).await, before);
}

#[tokio::test]
async fn unified_conversation_reopen_reuses_observed_execution_binding() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("observed.sqlite");
    let store = Store::open(&path).await.unwrap();
    let (conversation, run, root) = started_tree(&store).await;
    observe(&store, &run, &root, "root-native", &["child"])
        .await
        .unwrap();
    let children = store
        .list_child_conversations(conversation.id, None, 20)
        .await
        .unwrap()
        .items;
    store.close().await;
    let reopened = Store::open(&path).await.unwrap();
    observe(&reopened, &run, &root, "root-native", &["child"])
        .await
        .unwrap();
    assert_eq!(
        reopened
            .list_child_conversations(conversation.id, None, 20)
            .await
            .unwrap()
            .items,
        children
    );
}

#[tokio::test]
async fn unified_conversation_database_rejects_immutable_binding_and_invalid_parentage() {
    let store = Store::open_in_memory().await.unwrap();
    let (conversation, run, root) = started_tree(&store).await;
    observe(&store, &run, &root, "root-native", &["child"])
        .await
        .unwrap();
    let child = store
        .list_child_conversations(conversation.id, None, 20)
        .await
        .unwrap()
        .items
        .remove(0);
    let other = store
        .create_conversation(NewConversation::projectless("other owner"))
        .await
        .unwrap();
    let (other_run, _) = store
        .create_run(other.id, ProviderId::Claude)
        .await
        .unwrap();
    for (statement, value, id) in [
        (
            "UPDATE conversations SET parent_id = ? WHERE id = ?",
            child.id.to_string(),
            conversation.id.to_string(),
        ),
        (
            "UPDATE conversations SET parent_id = ? WHERE id = ?",
            other.id.to_string(),
            child.id.to_string(),
        ),
        (
            "UPDATE conversations SET observed_agent_id = ? WHERE id = ?",
            root.id.to_string(),
            child.id.to_string(),
        ),
        (
            "UPDATE agent_nodes SET parent_id = ? WHERE provider_native_id = ?",
            root.id.to_string(),
            "root-native".into(),
        ),
        (
            "UPDATE provider_runs SET conversation_id = ? WHERE id = ?",
            other.id.to_string(),
            run.id.to_string(),
        ),
        (
            "UPDATE provider_runs SET provider = ? WHERE id = ?",
            "claude".into(),
            run.id.to_string(),
        ),
        (
            "UPDATE agent_nodes SET provider = ? WHERE id = ?",
            "claude".into(),
            root.id.to_string(),
        ),
        (
            "UPDATE agent_nodes SET run_id = ? WHERE id = ?",
            other_run.id.to_string(),
            root.id.to_string(),
        ),
        (
            "INSERT INTO conversations (id, parent_id, title, status, created_at, updated_at) VALUES (?, ?, 'invalid', 'active', 1, 1)",
            ConversationId::new().to_string(),
            ConversationId::new().to_string(),
        ),
    ] {
        let mut transaction = store.pool.begin().await.unwrap();
        assert!(
            sqlx::query(statement)
                .bind(value)
                .bind(id)
                .execute(&mut *transaction)
                .await
                .is_err(),
            "accepted {statement}"
        );
        transaction.rollback().await.unwrap();
    }
    assert_eq!(
        store.load_conversation(child.id).await.unwrap().parent_id,
        Some(conversation.id)
    );
}

#[tokio::test]
async fn unified_conversation_exact_owner_reads_have_bounded_indexes() {
    let store = Store::open_in_memory().await.unwrap();
    for (query, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT id FROM events WHERE agent_id = 'fixture' AND sequence < 100 ORDER BY sequence DESC LIMIT 21",
            "idx_events_agent_sequence",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM approvals WHERE agent_id = 'fixture' AND status = 'pending' ORDER BY created_at DESC, id DESC LIMIT 21",
            "idx_approvals_agent_status_created",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM conversations WHERE parent_id = 'fixture' AND status = 'active' AND (created_at, id) < (100, 'fixture') ORDER BY created_at DESC, id DESC LIMIT 21",
            "idx_conversations_children",
        ),
    ] {
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(query).fetch_all(&store.pool).await.unwrap();
        assert!(
            plan.iter().any(|row| row.3.contains(index)),
            "missing {index}: {plan:?}"
        );
        assert!(
            !plan.iter().any(|row| row.3.contains("TEMP B-TREE")),
            "unbounded sorting: {plan:?}"
        );
    }
}

#[tokio::test]
async fn unified_conversation_migration_rejects_unrooted_ancestry_transactionally() {
    for corruption in ["cycle", "missing parent", "cross-run parent"] {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let fixture = legacy_fixture(&pool).await;
        let invalid_parent = match corruption {
            "cycle" => fixture.grandchild,
            "missing parent" => AgentId::new(),
            _ => legacy_fixture(&pool).await.root,
        };
        // Simulate malformed historical ancestry, including files written with FKs disabled.
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_nodes SET parent_id = ? WHERE id = ?")
            .bind(invalid_parent.to_string())
            .bind(fixture.child.to_string())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        let before = historical_snapshot(&pool).await;
        assert!(MIGRATOR.run(&pool).await.is_err(), "accepted {corruption}");
        assert_eq!(historical_snapshot(&pool).await, before);
        let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
            sqlx::query_as("PRAGMA table_info(conversations)")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(!columns.iter().any(|column| column.1 == "parent_id"));
    }
}
