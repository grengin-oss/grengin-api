// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Runs on the disposable security-test database: background model calls (extraction, summaries,
// embeddings) go through a fake provider that is neither "openai" nor "anthropic".

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use futures_util::stream;
use llm_plugin::{
    ChatCapabilities, ChatProvider, ChatRequest, ChatSession, EmbeddingProvider, EmbeddingRequest,
    EmbeddingResult, ModelId, ModelProvider, ProviderCapabilities, ProviderDescriptor,
    ProviderError, ProviderEvent, ProviderEventStream, ProviderId, ProviderModel,
    ProviderModelType, ProviderPlugin, TokenUsage, ToolResult,
};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};
use serde_json::json;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    config::setting::{AuthSettings, EmbeddingSettings, RagSettings, ServerSettings, Settings},
    models::{
        conversation_summaries, messages::ChatRole, project_sources, usage_logs,
        usage_logs::UsageLogStatus,
    },
    services::{
        client_ip::TrustedProxies,
        file_storage::{FileWrite, StorageCategory, store_file_bytes},
        llm_usage_gate::{ModelRates, UsageCharge, UsagePurpose, UsageSubject},
        project_source_processing::process_project_source,
        rag::{EmbeddingTarget, embed_messages, update_conversation_summary},
    },
    state::{AppState, SharedState},
};

const TEXT_MODEL: &str = "seer";
const EMBED_MODEL: &str = "etch";
const CHAT_INPUT_TOKENS: u32 = 100;
const CHAT_OUTPUT_TOKENS: u32 = 20;
const EMBED_TOKENS: u32 = 40;

fn chat_cost() -> Decimal {
    Decimal::from_str_exact("0.00036").unwrap()
}

fn embedding_cost() -> Decimal {
    Decimal::from_str_exact("0.00002").unwrap()
}

#[derive(Default)]
struct Calls {
    chat: AtomicUsize,
    embed: AtomicUsize,
    omit_usage: AtomicBool,
    omit_embedding_price: AtomicBool,
    invalid_embedding_price: AtomicBool,
    omit_text_price: AtomicBool,
    pause_chat: AtomicBool,
    chat_started: tokio::sync::Notify,
    chat_release: tokio::sync::Notify,
}

struct FakeProvider {
    descriptor: ProviderDescriptor,
    calls: Arc<Calls>,
    text_model: String,
}

impl ProviderPlugin for FakeProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn chat(&self) -> Option<&dyn ChatProvider> {
        Some(self)
    }

    fn embeddings(&self) -> Option<&dyn EmbeddingProvider> {
        Some(self)
    }

    fn images(&self) -> Option<&dyn llm_plugin::ImageProvider> {
        None
    }

    fn models(&self) -> Option<&dyn ModelProvider> {
        Some(self)
    }
}

#[async_trait]
impl ChatProvider for FakeProvider {
    async fn start(&self, request: ChatRequest) -> Result<Box<dyn ChatSession>, ProviderError> {
        assert_eq!(request.model.as_str(), self.text_model);
        self.calls.chat.fetch_add(1, Ordering::SeqCst);
        if self.calls.pause_chat.load(Ordering::SeqCst) {
            self.calls.chat_started.notify_one();
            self.calls.chat_release.notified().await;
        }
        Ok(Box::new(FakeSession {
            omit_usage: self.calls.omit_usage.load(Ordering::SeqCst),
        }))
    }
}

struct FakeSession {
    omit_usage: bool,
}

#[async_trait]
impl ChatSession for FakeSession {
    async fn stream(&mut self) -> Result<ProviderEventStream, ProviderError> {
        let mut events = vec![
            Ok(ProviderEvent::TextDelta {
                text: "Fixture notes about quarterly budgets and owners.".to_string(),
            }),
            Ok(ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: Some(CHAT_INPUT_TOKENS),
                    output_tokens: Some(CHAT_OUTPUT_TOKENS),
                    ..Default::default()
                },
            }),
            Ok(ProviderEvent::Completed {
                finish_reason: None,
            }),
        ];
        if self.omit_usage {
            events.retain(|event| !matches!(event, Ok(ProviderEvent::Usage { .. })));
        }
        Ok(Box::pin(stream::iter(events)))
    }

    async fn continue_with_tools(
        &mut self,
        _results: Vec<ToolResult>,
    ) -> Result<ProviderEventStream, ProviderError> {
        Err(ProviderError::UnsupportedCapability("tools"))
    }
}

#[async_trait]
impl EmbeddingProvider for FakeProvider {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResult, ProviderError> {
        assert_eq!(request.model.as_str(), EMBED_MODEL);
        self.calls.embed.fetch_add(1, Ordering::SeqCst);
        let dimensions = request.dimensions.unwrap_or(1536) as usize;
        Ok(EmbeddingResult {
            vectors: request
                .inputs
                .iter()
                .map(|_| vec![0.25; dimensions])
                .collect(),
            usage: Some(TokenUsage {
                input_tokens: Some(EMBED_TOKENS),
                ..Default::default()
            }),
        })
    }
}

#[async_trait]
impl ModelProvider for FakeProvider {
    async fn list_models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        Ok(self.static_models())
    }

    fn static_models(&self) -> Vec<ProviderModel> {
        vec![
            ProviderModel {
                id: ModelId::new(self.text_model.clone()),
                name: "Seer".to_string(),
                model_type: ProviderModelType::TextGenerator,
                capabilities: ProviderCapabilities {
                    chat: Some(ChatCapabilities {
                        streaming: true,
                        vision: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                metadata: if self.calls.omit_text_price.load(Ordering::SeqCst) {
                    json!({"supportsPdfNative":true})
                } else {
                    json!({
                        "inputTokenRate": 2.0,
                        "outputTokenRate": 8.0,
                        "supportsPdfNative": true,
                    })
                },
            },
            ProviderModel {
                id: ModelId::new(EMBED_MODEL),
                name: "Etch".to_string(),
                model_type: ProviderModelType::TextEmbedder,
                capabilities: ProviderCapabilities::default(),
                metadata: if self.calls.omit_embedding_price.load(Ordering::SeqCst) {
                    json!({})
                } else if self.calls.invalid_embedding_price.load(Ordering::SeqCst) {
                    json!({"inputTokenRate":-1.0})
                } else {
                    json!({ "inputTokenRate": 0.5 })
                },
            },
        ]
    }
}

struct Owner {
    user: Uuid,
    department: Uuid,
    engine_key: String,
}

fn charges_for(ledger: &[usage_logs::Model], purpose: &str) -> Vec<usage_logs::Model> {
    ledger
        .iter()
        .filter(|row| {
            row.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("purpose"))
                .and_then(|value| value.as_str())
                == Some(purpose)
        })
        .cloned()
        .collect()
}

fn assert_one_charge(rows: &[usage_logs::Model], owner: &Owner, model: &str, cost: Decimal) {
    assert_eq!(rows.len(), 1, "one charge per provider call: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.user_id, owner.user);
    assert_eq!(row.department, Some(owner.department.to_string()));
    assert_eq!(row.model_provider, owner.engine_key);
    assert_eq!(row.model_name, model);
    assert_eq!(row.status, UsageLogStatus::Charged);
    assert_eq!(row.cost_usd, cost);
}

struct Fixture {
    db: DatabaseConnection,
    state: SharedState,
    calls: Arc<Calls>,
    engine_key: String,
    engine_id: Uuid,
    user: Uuid,
    department: Uuid,
    storage_root: PathBuf,
}

impl Fixture {
    // A blocked department's allocation (budgetAllocated keeps two decimals) is already spent by an
    // earlier chat reply.
    async fn new(blocked: bool) -> Self {
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").expect("test database URL");
        assert!(
            url.contains("/grengin_security_test"),
            "refusing a non-test database"
        );
        let db = Database::connect(&url).await.expect("test database");
        let suffix = Uuid::new_v4().simple().to_string();
        let engine_key = format!("acme-{}", &suffix[..10]);
        let (engine_id, user, department) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let (allocated, action) = if blocked {
            ("1", "block")
        } else {
            ("100", "block")
        };
        db.execute_unprepared(&format!(
            r#"INSERT INTO departments (id, name, path, depth, "createdAt", "updatedAt", "budgetAllocated", "budgetPeriod", "actionOnExceed")
                 VALUES ('{department}', 'spend-{suffix}', 't{suffix}', 0, now(), now(), {allocated}, 'monthly', '{action}');
               INSERT INTO department_allowed_models (id, "departmentId", provider, model, "createdAt", "updatedAt")
                 VALUES ('{}', '{department}', '{engine_key}', '{TEXT_MODEL}', now(), now());
               INSERT INTO users (id, status, email, "emailVerified", "mfaEnabled", "createdAt", "updatedAt", "lastLoginAt", "departmentId")
                 VALUES ('{user}', 'active', 'spend-{user}@example.invalid', true, false, now(), now(), now(), '{department}');
               INSERT INTO ai_engines (id, "displayName", "isEnabled", "engineKey", "apiKeyStatus", "whitelistModels", "defaultModel", "createdAt", "updatedAt")
                 VALUES ('{engine_id}', 'Acme fixture', true, '{engine_key}', 'not_configured', ARRAY['{TEXT_MODEL}'], '{TEXT_MODEL}', now(), now());"#,
            Uuid::new_v4()
        ))
        .await
        .expect("fixture rows");

        let storage_root = std::env::temp_dir().join(format!("grengin-spend-{suffix}"));
        let settings = Settings {
            auth: AuthSettings {
                jwt_secret: "test-only".to_string(),
                app_key: [7; 32],
                redirect_url: "http://127.0.0.1".to_string(),
                database_url: url,
                auto_migrate: false,
                bootstrap_super_admin_email: None,
            },
            google: RwLock::new(None),
            azure: RwLock::new(None),
            server: ServerSettings {
                host: "127.0.0.1".to_string(),
                port: 0,
                trusted_proxies: TrustedProxies::from_setting(None).expect("loopback proxies"),
            },
            ai_engines_cache: RwLock::new(HashMap::new()),
            embedding: RwLock::new(Some(EmbeddingSettings {
                provider: engine_key.clone(),
                model: EMBED_MODEL.to_string(),
                dimensions: None,
                is_enabled: true,
            })),
            rag: RagSettings {
                enabled: true,
                summary_enabled: true,
                retrieval_enabled: true,
                recent_message_pairs: 1,
                retrieval_top_k: 3,
                max_context_tokens: 8000,
                summary_llm_provider: None,
                summary_llm_model: None,
            },
            file_storage_root: storage_root.clone(),
        };
        settings
            .set_ai_engine_cache(
                engine_key.clone(),
                None,
                true,
                vec![TEXT_MODEL.to_string(), EMBED_MODEL.to_string()],
            )
            .await;
        let state = AppState::for_tests(db.clone(), settings);
        let calls = Arc::new(Calls::default());
        state
            .provider_registry
            .register(Arc::new(FakeProvider {
                descriptor: ProviderDescriptor {
                    id: ProviderId::new(engine_key.clone()),
                    version: "1".to_string(),
                    name: "Acme".to_string(),
                    capabilities: ProviderCapabilities {
                        chat: Some(ChatCapabilities {
                            streaming: true,
                            vision: true,
                            ..Default::default()
                        }),
                        embeddings: true,
                        model_listing: true,
                        ..Default::default()
                    },
                },
                calls: calls.clone(),
                text_model: TEXT_MODEL.to_string(),
            }))
            .await;
        let fixture = Self {
            db,
            state,
            calls,
            engine_key,
            engine_id,
            user,
            department,
            storage_root,
        };
        if blocked {
            let conversation = fixture.conversation().await;
            fixture
                .message(conversation, ChatRole::Assistant, 0, "5")
                .await;
        }
        fixture
    }

    async fn conversation(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.db
            .execute_unprepared(&format!(
                r#"INSERT INTO conversations (id, "userId", "modelProvider", "modelName", "createdAt", "updatedAt", "messageCount", "totalTokens", "totalCost")
                     VALUES ('{id}', '{}', '{}', '{TEXT_MODEL}', now(), now(), 0, 0, 0)"#,
                self.user, self.engine_key
            ))
            .await
            .expect("conversation");
        id
    }

    async fn message(&self, conversation: Uuid, role: ChatRole, order: i64, cost: &str) -> Uuid {
        let id = Uuid::new_v4();
        let created_at = Utc::now() - Duration::minutes(10) + Duration::seconds(order);
        let role = match role {
            ChatRole::Assistant => "assistant",
            _ => "user",
        };
        self.db
            .execute_unprepared(&format!(
                r#"INSERT INTO messages (id, "conversationId", role, "messageContent", "modelProvider", "modelName", "requestTokens", "responseTokens", "toolsCalls", "toolsResults", "createdAt", "updatedAt", "totalTokens", latency, cost)
                     VALUES ('{id}', '{conversation}', '{role}', 'Message {order} about budgets', '{}', '{TEXT_MODEL}', 0, 0, '{{}}', '{{}}', '{}', '{}', 0, 0, {cost})"#,
                self.engine_key,
                created_at.to_rfc3339(),
                created_at.to_rfc3339()
            ))
            .await
            .expect("message");
        id
    }

    async fn ledger(&self) -> Vec<usage_logs::Model> {
        usage_logs::Entity::find()
            .filter(usage_logs::Column::UserId.eq(self.user))
            .order_by_asc(usage_logs::Column::Timestamp)
            .all(&self.db)
            .await
            .expect("ledger rows")
    }

    fn owner(&self) -> Owner {
        Owner {
            user: self.user,
            department: self.department,
            engine_key: self.engine_key.clone(),
        }
    }

    async fn project_source(&self) -> Uuid {
        let (project, source, file) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        self.db
            .execute_unprepared(&format!(
                r#"INSERT INTO projects (id, name, category, visibility, "ownerId", "createdAt", "updatedAt")
                     VALUES ('{project}', 'Spend fixture', 'general', 'private', '{}', now(), now())"#,
                self.user
            ))
            .await
            .expect("project");
        store_file_bytes(
            &self.db,
            &self.storage_root,
            self.user,
            FileWrite {
                id: file,
                category: StorageCategory::File,
                name: "scan.png",
                content_type: "image/png",
                bytes: b"\x89PNG\r\n\x1a\nfixture-image-bytes",
                content_sha256: None,
                description: None,
                metadata: None,
            },
        )
        .await
        .expect("stored image");
        self.db
            .execute_unprepared(&format!(
                r#"INSERT INTO project_sources (id, "projectId", "fileName", "fileType", "fileSize", origin, "uploadedAt", "fileId", "processingStatus")
                     VALUES ('{source}', '{project}', 'scan.png', 'image/png', 27, 'upload', now(), '{file}', 'pending')"#
            ))
            .await
            .expect("project source");
        process_project_source(self.state.clone(), self.user, source, project, file).await;
        source
    }

    async fn source_status(&self, source: Uuid) -> project_sources::Model {
        project_sources::Entity::find_by_id(source)
            .one(&self.db)
            .await
            .expect("source query")
            .expect("source row")
    }

    async fn cleanup(self) {
        let user = self.user;
        self.db
            .execute_unprepared(&format!(
                r#"DELETE FROM project_source_chunks WHERE "projectId" IN (SELECT id FROM projects WHERE "ownerId" = '{user}');
                   DELETE FROM project_sources WHERE "projectId" IN (SELECT id FROM projects WHERE "ownerId" = '{user}');
                   DELETE FROM projects WHERE "ownerId" = '{user}';
                   DELETE FROM files WHERE "userId" = '{user}';
                   DELETE FROM message_embeddings WHERE "conversationId" IN (SELECT id FROM conversations WHERE "userId" = '{user}');
                   DELETE FROM conversation_summaries WHERE "conversationId" IN (SELECT id FROM conversations WHERE "userId" = '{user}');
                   UPDATE messages SET "previousMessageId" = NULL WHERE "conversationId" IN (SELECT id FROM conversations WHERE "userId" = '{user}');
                   DELETE FROM messages WHERE "conversationId" IN (SELECT id FROM conversations WHERE "userId" = '{user}');
                   DELETE FROM conversations WHERE "userId" = '{user}';
                   DELETE FROM notifications WHERE "departmentId" = '{}';
                   DELETE FROM users WHERE id = '{user}';
                   DELETE FROM department_allowed_models WHERE "departmentId" = '{}';
                   DELETE FROM departments WHERE id = '{}';
                   DELETE FROM ai_engines WHERE id = '{}';"#,
                self.department, self.department, self.department, self.engine_id
            ))
            .await
            .expect("cleanup");
        let _ = tokio::fs::remove_dir_all(&self.storage_root).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn source_extraction_and_its_embedding_each_write_one_charge() {
    let fixture = Fixture::new(false).await;
    let owner = fixture.owner();
    let source = fixture.project_source().await;

    let status = fixture.source_status(source).await;
    let ledger = fixture.ledger().await;
    let calls = (
        fixture.calls.chat.load(Ordering::SeqCst),
        fixture.calls.embed.load(Ordering::SeqCst),
    );
    fixture.cleanup().await;
    assert_eq!(
        status.processing_status, "ready",
        "{:?}",
        status.processing_error
    );
    assert_eq!(calls, (1, 1));
    assert_eq!(ledger.len(), 2, "one charge per provider call");
    let extraction = charges_for(&ledger, "source_extraction");
    assert_one_charge(&extraction, &owner, TEXT_MODEL, chat_cost());
    assert_eq!(extraction[0].request_tokens, i64::from(CHAT_INPUT_TOKENS));
    assert_eq!(extraction[0].response_tokens, i64::from(CHAT_OUTPUT_TOKENS));
    assert_eq!(extraction[0].conversation_id, None);
    let embedding = charges_for(&ledger, "source_embedding");
    assert_one_charge(&embedding, &owner, EMBED_MODEL, embedding_cost());
    assert_eq!(embedding[0].request_tokens, i64::from(EMBED_TOKENS));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn a_blocked_department_cannot_extract_sources() {
    let fixture = Fixture::new(true).await;
    let before = fixture.ledger().await.len();
    let source = fixture.project_source().await;

    let status = fixture.source_status(source).await;
    let after = fixture.ledger().await.len();
    let calls = (
        fixture.calls.chat.load(Ordering::SeqCst),
        fixture.calls.embed.load(Ordering::SeqCst),
    );
    fixture.cleanup().await;
    assert_eq!(before, 1, "the earlier reply's spend");
    assert_eq!(status.processing_status, "error");
    assert!(
        status
            .processing_error
            .as_deref()
            .is_some_and(|error| error.contains("DepartmentBudgetExceeded")),
        "{:?}",
        status.processing_error
    );
    assert_eq!(calls, (0, 0), "no provider call once the budget blocks");
    assert_eq!(after, before);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn summaries_and_message_embeddings_are_charged_once_beside_message_spend() {
    let fixture = Fixture::new(false).await;
    let conversation = fixture.conversation().await;
    let mut targets = Vec::new();
    for (order, role, cost) in [
        (0, ChatRole::User, "0"),
        (1, ChatRole::Assistant, "0.01"),
        (2, ChatRole::User, "0"),
        (3, ChatRole::Assistant, "0"),
    ] {
        let message_id = fixture.message(conversation, role, order, cost).await;
        targets.push(EmbeddingTarget {
            message_id,
            conversation_id: conversation,
            role,
            content: format!("Message {order} about budgets"),
            created_at: Utc::now(),
        });
    }

    let summary = update_conversation_summary(
        &fixture.state,
        fixture.user,
        conversation,
        &fixture.engine_key,
        TEXT_MODEL,
    )
    .await;
    let embedded = embed_messages(&fixture.state, fixture.user, targets).await;

    let owner = fixture.owner();
    let ledger = fixture.ledger().await;
    let summary_row = conversation_summaries::Entity::find()
        .filter(conversation_summaries::Column::ConversationId.eq(conversation))
        .one(&fixture.db)
        .await
        .expect("summary query");
    let calls = (
        fixture.calls.chat.load(Ordering::SeqCst),
        fixture.calls.embed.load(Ordering::SeqCst),
    );
    fixture.cleanup().await;
    assert!(summary.is_ok(), "{summary:?}");
    assert!(embedded.is_ok(), "{embedded:?}");
    assert!(summary_row.is_some(), "the summary must be written");
    assert_eq!(calls, (1, 1));
    let message_rows = ledger
        .iter()
        .filter(|row| {
            row.metadata
                .as_ref()
                .is_some_and(|metadata| metadata.get("messageId").is_some())
        })
        .count();
    assert_eq!(message_rows, 1, "message spend stays with the trigger");
    assert_eq!(ledger.len(), 3);
    let summaries = charges_for(&ledger, "conversation_summary");
    assert_one_charge(&summaries, &owner, TEXT_MODEL, chat_cost());
    assert_eq!(summaries[0].conversation_id, Some(conversation));
    let embeddings = charges_for(&ledger, "message_embedding");
    assert_one_charge(&embeddings, &owner, EMBED_MODEL, embedding_cost());
    assert_eq!(embeddings[0].conversation_id, Some(conversation));
    let total: Decimal = ledger.iter().map(|row| row.cost_usd).sum();
    assert_eq!(
        total,
        Decimal::from_str_exact("0.01").unwrap() + chat_cost() + embedding_cost()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn a_blocked_department_skips_summaries_and_embeddings_quietly() {
    let fixture = Fixture::new(true).await;
    let conversation = fixture.conversation().await;
    let mut targets = Vec::new();
    for order in 0..4 {
        let role = if order % 2 == 0 {
            ChatRole::User
        } else {
            ChatRole::Assistant
        };
        let message_id = fixture.message(conversation, role, order, "0").await;
        targets.push(EmbeddingTarget {
            message_id,
            conversation_id: conversation,
            role,
            content: format!("Message {order} about budgets"),
            created_at: Utc::now(),
        });
    }
    let before = fixture.ledger().await.len();

    let summary = update_conversation_summary(
        &fixture.state,
        fixture.user,
        conversation,
        &fixture.engine_key,
        TEXT_MODEL,
    )
    .await;
    let embedded = embed_messages(&fixture.state, fixture.user, targets).await;

    let after = fixture.ledger().await.len();
    let summary_row = conversation_summaries::Entity::find()
        .filter(conversation_summaries::Column::ConversationId.eq(conversation))
        .one(&fixture.db)
        .await
        .expect("summary query");
    let calls = (
        fixture.calls.chat.load(Ordering::SeqCst),
        fixture.calls.embed.load(Ordering::SeqCst),
    );
    fixture.cleanup().await;
    assert!(
        summary.is_ok(),
        "a refused summary is skipped, not an error"
    );
    assert!(
        embedded.is_ok(),
        "a refused embedding is skipped, not an error"
    );
    assert!(summary_row.is_none());
    assert_eq!(calls, (0, 0));
    assert_eq!(after, before);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn an_extraction_without_reported_usage_is_charged_an_estimate_not_zero() {
    let fixture = Fixture::new(false).await;
    fixture.calls.omit_usage.store(true, Ordering::SeqCst);
    let source = fixture.project_source().await;

    let status = fixture.source_status(source).await;
    let ledger = fixture.ledger().await;
    let calls = fixture.calls.chat.load(Ordering::SeqCst);
    fixture.cleanup().await;
    assert_eq!(
        status.processing_status, "ready",
        "{:?}",
        status.processing_error
    );
    assert_eq!(calls, 1);
    let extraction = charges_for(&ledger, "source_extraction");
    assert_eq!(extraction.len(), 1);
    assert!(
        extraction[0].cost_usd > Decimal::ZERO,
        "{:?}",
        extraction[0]
    );
    assert!(
        extraction[0].request_tokens >= 1_600,
        "the image is estimated"
    );
    assert!(extraction[0].response_tokens > 0, "the reply is estimated");
    assert_eq!(
        extraction[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("estimatedTokens")),
        Some(&json!(true))
    );
}

fn summary_charge(fixture: &Fixture, user: Uuid, conversation: Option<Uuid>) -> UsageCharge {
    UsageCharge::pre_authorized(
        UsageSubject {
            user_id: user,
            purpose: UsagePurpose::ConversationSummary,
            conversation_id: conversation,
        },
        Some(fixture.department),
        &fixture.engine_key,
        TEXT_MODEL,
        ModelRates {
            input: Some(2.0),
            output: Some(8.0),
        },
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn deleting_the_conversation_first_does_not_lose_its_charge() {
    let fixture = Fixture::new(false).await;
    let conversation = fixture.conversation().await;
    let charge = summary_charge(&fixture, fixture.user, Some(conversation));
    fixture
        .db
        .execute_unprepared(&format!(
            "DELETE FROM conversations WHERE id = '{conversation}'"
        ))
        .await
        .expect("delete conversation");

    let recorded = charge
        .record(
            &fixture.state,
            crate::services::llm_usage_gate::TokenCounts::reported(100, 20),
        )
        .await;
    let ledger = fixture.ledger().await;
    fixture.cleanup().await;
    assert!(recorded.is_ok(), "{recorded:?}");
    assert_eq!(ledger.len(), 1, "the charge must survive the deletion");
    assert_eq!(ledger[0].conversation_id, None);
    assert_eq!(ledger[0].cost_usd, chat_cost());
    assert_eq!(
        ledger[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("conversationId")),
        Some(&json!(conversation))
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn recording_one_charge_twice_writes_one_row() {
    let fixture = Fixture::new(false).await;
    let conversation = fixture.conversation().await;
    let charge = summary_charge(&fixture, fixture.user, Some(conversation));
    let tokens = crate::services::llm_usage_gate::TokenCounts::reported(100, 20);

    let first = charge.record(&fixture.state, tokens).await;
    let second = charge.record(&fixture.state, tokens).await;
    let ledger = fixture.ledger().await;
    fixture.cleanup().await;
    assert!(first.is_ok() && second.is_ok(), "{first:?} {second:?}");
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].conversation_id, Some(conversation));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn a_charge_that_cannot_be_written_is_reported_to_the_caller() {
    let fixture = Fixture::new(false).await;
    let missing_user = Uuid::new_v4();
    let charge = summary_charge(&fixture, missing_user, None);

    let recorded = charge
        .record(
            &fixture.state,
            crate::services::llm_usage_gate::TokenCounts::reported(100, 20),
        )
        .await;
    let rows = usage_logs::Entity::find()
        .filter(usage_logs::Column::UserId.eq(missing_user))
        .all(&fixture.db)
        .await
        .expect("ledger rows")
        .len();
    fixture.cleanup().await;
    assert!(
        matches!(recorded, Err(crate::error::AppError::DbTimeout)),
        "{recorded:?}"
    );
    assert_eq!(rows, 0);
}

// The first insert commits, then the caller is told the connection dropped, as when a commit's
// acknowledgement is lost. Retrying must not charge twice.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn a_committed_write_whose_acknowledgement_is_lost_is_not_charged_twice() {
    use sea_orm::{DbErr, RuntimeErr};
    let fixture = Fixture::new(false).await;
    let conversation = fixture.conversation().await;
    let charge = summary_charge(&fixture, fixture.user, Some(conversation));
    let tokens = crate::services::llm_usage_gate::TokenCounts::reported(100, 20);
    let attempts = AtomicUsize::new(0);
    let (db, charge_ref, attempts_ref) = (&fixture.state.database, &charge, &attempts);

    let written = charge
        .write_with_retries(move |conversation_id| {
            let attempt = attempts_ref.fetch_add(1, Ordering::SeqCst);
            async move {
                charge_ref.insert_row(db, tokens, conversation_id).await?;
                if attempt == 0 {
                    return Err(DbErr::Conn(RuntimeErr::Internal(
                        "connection closed after commit".to_string(),
                    )));
                }
                Ok(())
            }
        })
        .await;
    let ledger = fixture.ledger().await;
    let attempts = attempts.load(Ordering::SeqCst);
    fixture.cleanup().await;
    assert!(written.is_ok(), "{written:?}");
    assert_eq!(attempts, 2, "the lost acknowledgement is retried");
    assert_eq!(ledger.len(), 1, "the retry must not add a second charge");
    assert_eq!(ledger[0].cost_usd, chat_cost());
}

async fn start_new_chat(fixture: &Fixture) -> bool {
    start_new_chat_for(fixture, &fixture.engine_key, TEXT_MODEL).await
}

async fn start_new_chat_for(fixture: &Fixture, provider: &str, model: &str) -> bool {
    start_chat_for(fixture, provider, model, None).await
}

async fn start_chat_for(
    fixture: &Fixture,
    provider: &str,
    model: &str,
    conversation: Option<Uuid>,
) -> bool {
    use axum::{Json, extract::State};
    let input = serde_json::from_value(json!({
        "provider": provider,
        "model_name": model,
        "messages": [{ "role": "user", "content": "Plan a budget", "files": [] }]
    }))
    .expect("chat input");
    let result = crate::handlers::chat_stream::handle_chat_stream(
        crate::auth::claims::Claims {
            sub: "spend@example.invalid".into(),
            name: None,
            user_id: fixture.user,
            exp: usize::MAX,
        },
        conversation.map(axum::extract::Path),
        State(fixture.state.clone()),
        None,
        Json(input),
    )
    .await;
    match result {
        Ok(response) => {
            use axum::response::IntoResponse;
            let body = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                axum::body::to_bytes(response.into_response().into_body(), 1024 * 1024),
            )
            .await
            .expect("chat finishes")
            .expect("chat body");
            let text = String::from_utf8(body.to_vec()).unwrap();
            assert!(!text.contains("ai_error"), "chat failed: {text}");
            true
        }
        Err(crate::error::AppError::DbTimeout) => false,
        Err(error) => panic!("unexpected chat error: {error:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn gateway_chats_do_not_generate_or_charge_a_background_title() {
    let fixture = Fixture::new(false).await;
    let model = "openai/seer";
    fixture.db.execute_unprepared(&format!(
        r#"UPDATE ai_engines SET "engineKey" = 'grengin', "defaultModel" = '{model}', "whitelistModels" = ARRAY['{model}'] WHERE id = '{}';
           UPDATE department_allowed_models SET provider = 'grengin', model = '{model}' WHERE "departmentId" = '{}';"#,
        fixture.engine_id, fixture.department
    )).await.expect("gateway engine");
    fixture
        .state
        .settings
        .set_ai_engine_cache("grengin".into(), None, true, vec![model.to_string()])
        .await;
    let mut descriptor = fixture
        .state
        .provider_registry
        .get_by_str(&fixture.engine_key)
        .await
        .unwrap()
        .descriptor()
        .clone();
    descriptor.id = ProviderId::new("grengin");
    fixture
        .state
        .provider_registry
        .register(Arc::new(FakeProvider {
            descriptor,
            calls: fixture.calls.clone(),
            text_model: model.to_string(),
        }))
        .await;

    let started = start_new_chat_for(&fixture, "grengin", model).await;
    let conversations = user_conversations(&fixture).await;
    let ledger = fixture.ledger().await;
    let calls = fixture.calls.chat.load(Ordering::SeqCst);
    fixture.cleanup().await;
    assert!(started);
    assert_eq!(
        calls, 1,
        "only the requested answer, never a background title, may use the gateway"
    );
    assert!(
        charges_for(&ledger, "title_generation").is_empty(),
        "a gateway title must not create a local charge"
    );
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].title.as_deref(), Some("Plan a budget"));
    assert_eq!(
        conversations[0].metadata.as_ref().unwrap()["fundingSource"],
        "grengin"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn project_retrieval_reuses_one_billed_query_and_keeps_source_names_private() {
    use crate::services::rag::{build_project_retrieval_prompt, embed_retrieval_query};

    let fixture = Fixture::new(false).await;
    let source_id = fixture.project_source().await;
    let source = fixture.source_status(source_id).await;
    assert_eq!(source.processing_status, "ready");
    let conversation = fixture.conversation().await;
    let query = embed_retrieval_query(
        &fixture.state,
        fixture.user,
        conversation,
        "quarterly budgets",
    )
    .await
    .unwrap()
    .expect("shared query");
    let private =
        build_project_retrieval_prompt(&fixture.state, source.project_id, &query, 3, false)
            .await
            .unwrap()
            .expect("private project context");
    let named = build_project_retrieval_prompt(&fixture.state, source.project_id, &query, 3, true)
        .await
        .unwrap()
        .expect("named project context");
    let ledger = fixture.ledger().await;
    fixture.cleanup().await;
    assert!(private.contains("Fixture notes"));
    assert!(!private.contains("scan.png"));
    assert!(named.contains("scan.png"));
    let queries = charges_for(&ledger, "query_embedding");
    assert_eq!(queries.len(), 1);
    assert!(queries.iter().all(|row| row.cost_usd == embedding_cost()));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn own_key_embeddings_are_priced_and_charged_under_the_resolved_model() {
    use crate::services::rag::generate_embeddings;

    let fixture = Fixture::new(false).await;
    let gateway_id = Uuid::new_v4();
    let qualified = format!("openai/{EMBED_MODEL}");
    fixture.db.execute_unprepared(&format!(
        r#"UPDATE ai_engines SET "engineKey" = 'openai', "apiKey" = 'fixture-only', "whitelistModels" = ARRAY['{EMBED_MODEL}'] WHERE id = '{}';
           INSERT INTO ai_engines (id, "displayName", "isEnabled", "engineKey", "apiKeyStatus", "whitelistModels", "defaultModel", "createdAt", "updatedAt")
             VALUES ('{gateway_id}', 'Gateway fixture', true, 'grengin', 'not_configured', ARRAY['{qualified}'], '{qualified}', now(), now());"#,
        fixture.engine_id
    )).await.expect("own-key routing fixtures");
    let mut descriptor = fixture
        .state
        .provider_registry
        .get_by_str(&fixture.engine_key)
        .await
        .unwrap()
        .descriptor()
        .clone();
    descriptor.id = ProviderId::new("openai");
    fixture
        .state
        .provider_registry
        .register(Arc::new(FakeProvider {
            descriptor,
            calls: fixture.calls.clone(),
            text_model: TEXT_MODEL.to_string(),
        }))
        .await;
    let result = generate_embeddings(
        &fixture.state,
        &EmbeddingSettings {
            provider: "grengin".into(),
            model: qualified,
            dimensions: Some(1536),
            is_enabled: true,
        },
        UsageSubject {
            user_id: fixture.user,
            purpose: UsagePurpose::QueryEmbedding,
            conversation_id: None,
        },
        vec!["A query about budgets".into()],
    )
    .await;
    let ledger = fixture.ledger().await;
    let calls = fixture.calls.embed.load(Ordering::SeqCst);
    fixture
        .db
        .execute_unprepared(&format!("DELETE FROM ai_engines WHERE id = '{gateway_id}'"))
        .await
        .expect("remove gateway fixture");
    fixture.cleanup().await;
    assert!(result.unwrap().is_some());
    assert_eq!(calls, 1);
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].model_provider, "openai");
    assert_eq!(ledger[0].model_name, EMBED_MODEL);
    assert_eq!(ledger[0].cost_usd, embedding_cost());
}

async fn user_conversations(fixture: &Fixture) -> Vec<crate::models::conversations::Model> {
    crate::models::conversations::Entity::find()
        .filter(crate::models::conversations::Column::UserId.eq(fixture.user))
        .all(&fixture.db)
        .await
        .expect("conversations")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn unpriced_embeddings_run_for_budgeted_uploads_and_are_explicitly_marked_free() {
    let fixture = Fixture::new(false).await;
    fixture
        .calls
        .omit_embedding_price
        .store(true, Ordering::SeqCst);
    let source = fixture.project_source().await;
    let status = fixture.source_status(source).await;
    let ledger = fixture.ledger().await;
    fixture.cleanup().await;
    assert_eq!(status.processing_status, "ready");
    let embeddings = charges_for(&ledger, "source_embedding");
    assert_eq!(embeddings.len(), 1);
    assert_eq!(embeddings[0].cost_usd, Decimal::ZERO);
    assert_eq!(
        embeddings[0].metadata.as_ref().unwrap()["unpricedEmbedding"],
        true
    );
    assert_eq!(
        charges_for(&ledger, "source_extraction")[0].cost_usd,
        chat_cost()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn unpriced_text_models_and_invalid_embedding_prices_are_not_made_free() {
    let fixture = Fixture::new(false).await;
    fixture.calls.omit_text_price.store(true, Ordering::SeqCst);
    let source = fixture.project_source().await;
    assert_eq!(
        fixture.source_status(source).await.processing_status,
        "error"
    );
    assert_eq!(fixture.calls.chat.load(Ordering::SeqCst), 0);
    fixture
        .calls
        .invalid_embedding_price
        .store(true, Ordering::SeqCst);
    let config = fixture.state.settings.get_embedding_config().await.unwrap();
    let result = crate::services::rag::generate_embeddings(
        &fixture.state,
        &config,
        UsageSubject {
            user_id: fixture.user,
            purpose: UsagePurpose::QueryEmbedding,
            conversation_id: None,
        },
        vec!["query".into()],
    )
    .await;
    let ledger = fixture.ledger().await;
    let calls = fixture.calls.embed.load(Ordering::SeqCst);
    fixture.cleanup().await;
    assert!(result.is_err());
    assert_eq!(calls, 0);
    assert!(ledger.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn query_charge_failure_discards_context_but_keeps_the_message() {
    let fixture = Fixture::new(false).await;
    let conversation = fixture.conversation().await;
    let trigger = format!("reject_query_{}", fixture.user.simple());
    fixture.db.execute_unprepared(&format!(r#"CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW."userId" = '{}' AND NEW.metadata->>'purpose' = 'query_embedding' THEN RAISE EXCEPTION 'ledger unavailable'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER {trigger} BEFORE INSERT ON usage_logs FOR EACH ROW EXECUTE FUNCTION {trigger}();"#, fixture.user)).await.unwrap();
    let started = start_chat_for(
        &fixture,
        &fixture.engine_key,
        TEXT_MODEL,
        Some(conversation),
    )
    .await;
    let rows = crate::models::messages::Entity::find()
        .filter(crate::models::messages::Column::ConversationId.eq(conversation))
        .all(&fixture.db)
        .await
        .unwrap();
    let ledger = fixture.ledger().await;
    let calls = fixture.calls.embed.load(Ordering::SeqCst);
    fixture
        .db
        .execute_unprepared(&format!(
            "DROP TRIGGER {trigger} ON usage_logs; DROP FUNCTION {trigger}();"
        ))
        .await
        .unwrap();
    fixture.cleanup().await;
    assert!(started);
    assert!(calls >= 1);
    assert!(charges_for(&ledger, "query_embedding").is_empty());
    assert!(rows.iter().any(
        |message| message.role == ChatRole::User && message.message_content == "Plan a budget"
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn a_chat_with_five_linked_projects_embeds_and_charges_the_query_once() {
    let fixture = Fixture::new(false).await;
    let conversation = fixture.conversation().await;
    for _ in 0..5 {
        let source = fixture.project_source().await;
        let project = fixture.source_status(source).await.project_id;
        fixture.db.execute_unprepared(&format!(r#"INSERT INTO conversation_projects (id, "conversationId", "projectId", "createdAt")
            VALUES ('{}', '{conversation}', '{project}', now())"#, Uuid::new_v4())).await.unwrap();
    }
    let before = fixture.calls.embed.load(Ordering::SeqCst);
    let started = start_chat_for(
        &fixture,
        &fixture.engine_key,
        TEXT_MODEL,
        Some(conversation),
    )
    .await;
    let after = fixture.calls.embed.load(Ordering::SeqCst);
    let ledger = fixture.ledger().await;
    fixture.cleanup().await;
    assert!(started);
    assert!(after > before);
    assert_eq!(charges_for(&ledger, "query_embedding").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn disallowed_summary_preferences_fall_back_to_an_allowed_model() {
    use sea_orm::{ActiveModelTrait, IntoActiveModel, Set};
    for denied_by_department in [false, true] {
        let mut fixture = Fixture::new(false).await;
        let alternate_id = Uuid::new_v4();
        let alternate_key = format!("summary-{}", alternate_id.simple());
        let mut alternate = crate::models::ai_engines::Entity::find_by_id(fixture.engine_id)
            .one(&fixture.db)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        alternate.id = Set(alternate_id);
        alternate.engine_key = Set(alternate_key.clone());
        alternate.created_at = Set(Utc::now() + Duration::seconds(1));
        alternate.insert(&fixture.db).await.unwrap();
        let mut descriptor = fixture
            .state
            .provider_registry
            .get_by_str(&fixture.engine_key)
            .await
            .unwrap()
            .descriptor()
            .clone();
        descriptor.id = ProviderId::new(alternate_key.clone());
        fixture
            .state
            .provider_registry
            .register(Arc::new(FakeProvider {
                descriptor,
                calls: fixture.calls.clone(),
                text_model: TEXT_MODEL.into(),
            }))
            .await;
        fixture
            .state
            .settings
            .set_ai_engine_cache(alternate_key.clone(), None, true, vec![TEXT_MODEL.into()])
            .await;
        let rag = &mut Arc::get_mut(&mut fixture.state).unwrap().settings.rag;
        rag.summary_llm_provider = Some(if denied_by_department {
            alternate_key
        } else {
            fixture.engine_key.clone()
        });
        rag.summary_llm_model = Some(if denied_by_department {
            TEXT_MODEL.into()
        } else {
            "not-whitelisted".into()
        });
        let conversation = fixture.conversation().await;
        for index in 0..4 {
            fixture
                .message(conversation, ChatRole::User, index, "0")
                .await;
        }
        update_conversation_summary(
            &fixture.state,
            fixture.user,
            conversation,
            &fixture.engine_key,
            TEXT_MODEL,
        )
        .await
        .unwrap();
        let summary = conversation_summaries::Entity::find()
            .filter(conversation_summaries::Column::ConversationId.eq(conversation))
            .one(&fixture.db)
            .await
            .unwrap();
        let ledger = fixture.ledger().await;
        crate::models::ai_engines::Entity::delete_by_id(alternate_id)
            .exec(&fixture.db)
            .await
            .unwrap();
        let engine_key = fixture.engine_key.clone();
        fixture.cleanup().await;
        assert!(summary.is_some());
        assert_eq!(
            charges_for(&ledger, "conversation_summary")[0].model_provider,
            engine_key
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn a_generated_title_is_published_once_charged() {
    let fixture = Fixture::new(false).await;

    let started = start_new_chat(&fixture).await;
    let conversations = user_conversations(&fixture).await;
    let ledger = fixture.ledger().await;
    let owner = fixture.owner();
    fixture.cleanup().await;
    assert!(started);
    assert_eq!(conversations.len(), 1);
    assert_eq!(
        conversations[0].title.as_deref(),
        Some("Fixture notes about quarterly budgets and owners.")
    );
    let titles = charges_for(&ledger, "title_generation");
    assert_one_charge(&titles, &owner, TEXT_MODEL, chat_cost());
    assert_eq!(titles[0].conversation_id, Some(conversations[0].id));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn title_publication_failure_keeps_the_charged_chat_and_user_message() {
    let fixture = Fixture::new(false).await;
    let trigger = format!("reject_title_{}", fixture.user.simple());
    fixture.db.execute_unprepared(&format!(
        r#"CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$
          BEGIN IF NEW."userId" = '{}' THEN RAISE EXCEPTION 'title unavailable'; END IF; RETURN NEW; END $$;
          CREATE TRIGGER {trigger} BEFORE UPDATE OF title ON conversations FOR EACH ROW EXECUTE FUNCTION {trigger}();"#,
        fixture.user
    )).await.unwrap();
    let started = start_new_chat(&fixture).await;
    let conversations = user_conversations(&fixture).await;
    let ledger = fixture.ledger().await;
    let messages = crate::models::messages::Entity::find()
        .filter(crate::models::messages::Column::ConversationId.eq(conversations[0].id))
        .all(&fixture.db)
        .await
        .unwrap();
    fixture
        .db
        .execute_unprepared(&format!(
            "DROP TRIGGER {trigger} ON conversations; DROP FUNCTION {trigger}();"
        ))
        .await
        .unwrap();
    fixture.cleanup().await;
    assert!(started);
    assert_eq!(conversations[0].title.as_deref(), Some("Plan a budget"));
    assert_eq!(charges_for(&ledger, "title_generation").len(), 1);
    assert!(messages.iter().any(
        |message| message.role == ChatRole::User && message.message_content == "Plan a budget"
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn deleting_a_summarized_message_drops_the_summary_and_preserves_spend() {
    let fixture = Fixture::new(false).await;
    let chat = fixture.conversation().await;
    let first = fixture.message(chat, ChatRole::User, 0, "0").await;
    for order in 1..4 {
        fixture
            .message(chat, ChatRole::Assistant, order, "0.01")
            .await;
    }
    update_conversation_summary(
        &fixture.state,
        fixture.user,
        chat,
        &fixture.engine_key,
        TEXT_MODEL,
    )
    .await
    .unwrap();
    assert!(
        crate::services::rag::load_summary(&fixture.db, chat)
            .await
            .unwrap()
            .is_some()
    );
    let before: Decimal = fixture.ledger().await.iter().map(|row| row.cost_usd).sum();
    crate::services::message_helpers::soft_delete_message(&fixture.db, fixture.user, chat, first)
        .await
        .unwrap();
    let summary = crate::services::rag::load_summary(&fixture.db, chat)
        .await
        .unwrap();
    let after: Decimal = fixture.ledger().await.iter().map(|row| row.cost_usd).sum();
    fixture.cleanup().await;
    assert!(summary.is_none());
    assert_eq!(before, after);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn deleting_any_source_or_the_old_summary_while_summarizing_cannot_resurrect_it() {
    for has_summary in [false, true] {
        let fixture = Fixture::new(false).await;
        let chat = fixture.conversation().await;
        let first = fixture.message(chat, ChatRole::User, 0, "0").await;
        for order in 1..4 {
            fixture.message(chat, ChatRole::Assistant, order, "0").await;
        }
        if has_summary {
            update_conversation_summary(
                &fixture.state,
                fixture.user,
                chat,
                &fixture.engine_key,
                TEXT_MODEL,
            )
            .await
            .unwrap();
            for order in 4..8 {
                fixture.message(chat, ChatRole::Assistant, order, "0").await;
            }
        }
        fixture.calls.pause_chat.store(true, Ordering::SeqCst);
        let (state, user, engine) = (
            fixture.state.clone(),
            fixture.user,
            fixture.engine_key.clone(),
        );
        let generating = tokio::spawn(async move {
            update_conversation_summary(&state, user, chat, &engine, TEXT_MODEL).await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.calls.chat_started.notified(),
        )
        .await
        .unwrap();
        crate::services::message_helpers::soft_delete_message(
            &fixture.db,
            fixture.user,
            chat,
            first,
        )
        .await
        .unwrap();
        fixture.calls.chat_release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), generating)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let summary = crate::services::rag::load_summary(&fixture.db, chat)
            .await
            .unwrap();
        fixture.cleanup().await;
        assert!(
            summary.is_none(),
            "a deleted earlier message must not be retained just because the newest source is still visible"
        );
    }
}

// Only the optional title's charge fails; the main answer must still complete and be charged.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
async fn title_charge_failure_keeps_the_message_and_only_the_fallback_title() {
    let fixture = Fixture::new(false).await;
    let trigger = format!("reject_ledger_{}", fixture.user.simple());
    fixture
        .db
        .execute_unprepared(&format!(
            r#"CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN IF NEW."userId" = '{}' AND NEW.metadata->>'purpose' = 'title_generation' THEN RAISE EXCEPTION 'ledger unavailable'; END IF; RETURN NEW; END $$;
               CREATE TRIGGER {trigger} BEFORE INSERT ON usage_logs FOR EACH ROW EXECUTE FUNCTION {trigger}();"#,
            fixture.user
        ))
        .await
        .expect("failing ledger");

    let started = start_new_chat(&fixture).await;
    let conversations = user_conversations(&fixture).await;
    let ledger = fixture.ledger().await;
    let calls = fixture.calls.chat.load(Ordering::SeqCst);
    let user_messages = crate::models::messages::Entity::find()
        .filter(crate::models::messages::Column::Role.eq(ChatRole::User))
        .all(&fixture.db)
        .await
        .unwrap();
    fixture
        .db
        .execute_unprepared(&format!(
            "DROP TRIGGER {trigger} ON usage_logs; DROP FUNCTION {trigger}();"
        ))
        .await
        .expect("restore ledger");
    fixture.cleanup().await;
    assert!(
        started,
        "a failed optional charge must not fail the message"
    );
    assert_eq!(calls, 2);
    assert!(charges_for(&ledger, "title_generation").is_empty());
    assert!(ledger.iter().any(|row| {
        row.metadata
            .as_ref()
            .is_some_and(|metadata| metadata.get("messageId").is_some())
    }));
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].title.as_deref(), Some("Plan a budget"));
    assert!(
        user_messages
            .iter()
            .any(|message| message.conversation_id == conversations[0].id
                && message.message_content == "Plan a budget")
    );
}
