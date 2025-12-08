use utoipa::OpenApi;
use crate::auth::claims::Claims;
use crate::dto::chat::{ArchiveChatRequest, ChatStream, ConversationResponse, File, MessageParts, MessageResponse, TokenUsage};
use crate::error::{ErrorResponse, ErrorDetail, ErrorDetailVariant};
use crate::docs::security::ApiSecurityAddon;
use crate::dto::auth::{AuthInitResponse, AuthTokenResponse, LoginResponse, TokenType, User};
use crate::handlers::{oidc,chat,chat_stream};
use crate::models::users::{UserRole, UserStatus};

#[derive(OpenApi)]
#[openapi(
    paths(
        oidc::oidc_login_start,
        oidc::oidc_oauth_callback,
        chat::get_chat_by_id,
        chat::get_chats,
        chat::delete_chat_by_id,
        chat::update_chat_by_id,
        chat_stream::handle_chat_stream,
    ),
    components(
        schemas(
            LoginResponse,
            AuthInitResponse,
            AuthTokenResponse,
            TokenType,
            User,
            UserRole,
            UserStatus,
            Claims,
            ErrorResponse,
            ErrorDetail,
            ErrorDetailVariant,
            ArchiveChatRequest,
            MessageResponse,
            ConversationResponse,
            File,
            MessageParts,
            TokenUsage,
            ChatStream
        )
    ),
    tags(
        (name = "auth", description = "Authentication & user endpoints"),
        (name = "root", description = "Root / health"),
    ),
    modifiers(
        &ApiSecurityAddon
    )
)]
pub struct ApiDoc;