use utoipa::OpenApi;
use crate::auth::claims::Claims;
use crate::dto::admin_ai::{AiEngineResponse, AiEngineUpdateRequest};
use crate::dto::admin_org::OrgResponse;
use crate::dto::admin_user::{UserDetails, UserPatchRequest, UserRequest, UserResponse, UserUpdateRequest};
use crate::dto::chat::{ArchiveChatRequest, ConversationResponse, MessageParts, MessageResponse, TokenUsage};
use crate::dto::chat_stream::{ChatInitRequest, ChatStream};
use crate::dto::common::{PaginationQuery, SortRule};
use crate::dto::files::{File,Attachment};
use crate::dto::oauth::OAuthCallback;
use crate::error::{ErrorResponse, ErrorDetail, ErrorDetailVariant};
use crate::docs::security::ApiSecurityAddon;
use crate::dto::auth::{AuthInitResponse, AuthTokenResponse, LoginResponse, TokenType, User};
use crate::handlers::{oidc,chat,chat_stream,files,message,admin_users,admin_org,admin_ai};
use crate::models::messages::ChatRole;
use crate::models::users::{UserRole, UserStatus};

#[derive(OpenApi)]
#[openapi(
    paths(
        oidc::oidc_login_start,
        oidc::oidc_oauth_callback_get,
        oidc::oidc_oauth_callback_post,
        chat::get_chat_by_id,
        chat::get_chats,
        chat::delete_chat_by_id,
        chat::update_chat_by_id,
        chat_stream::handle_chat_stream_doc,
        chat_stream::handle_chat_stream_path_doc,
        files::upload_file,
        message::delete_chat_message_by_id,
        message::edit_chat_message_by_id_and_stream,
        admin_users::add_new_user,
        admin_users::get_users,
        admin_users::update_user,
        admin_users::delete_user,
        admin_users::get_user_by_id,
        admin_users::patch_user_status,
        admin_org::get_org,
        admin_org::update_org,
        admin_ai::get_ai_engines,
        admin_ai::update_ai_engines_by_key,
        admin_ai::get_ai_engines_by_key,
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
            ChatRole,
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
            ChatStream,
            ChatInitRequest,
            Attachment,
            OAuthCallback,
            SortRule,
            PaginationQuery,
            UserResponse,
            UserUpdateRequest,
            UserRequest,
            OrgResponse,
            UserDetails,
            UserPatchRequest,
            AiEngineResponse,
            AiEngineUpdateRequest,
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