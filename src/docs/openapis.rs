use utoipa::OpenApi;
use crate::auth::claims::Claims;
use crate::auth::error::AuthError;
use crate::dto::admin_ai::{AiEngineResponse, AiEngineUpdateRequest, AiEngineValidationResponse, AiModel,AiEngineModelsResponse, AiModelCapabilities};
use crate::dto::admin_department::{Department, DepartmentResponse};
use crate::dto::admin_org::OrgResponse;
use crate::dto::admin_sso_providers::{SsoProviderResponse, SsoProviderUpdateRequest};
use crate::dto::admin_user::{UserDetails, UserPatchRequest, UserRequest, UserResponse, UserUpdateRequest};
use crate::dto::chat::{ArchiveChatRequest, ConversationResponse, MessageParts, MessageResponse, TokenUsage};
use crate::dto::chat_stream::{ChatInitRequest, ChatStream};
use crate::dto::common::{PaginationQuery, SortRule};
use crate::dto::files::{Attachment, File, FileResponse, FileUploadRequest};
use crate::dto::models::{ModelInfo, ProviderInfo};
use crate::dto::oauth::OAuthCallback;
use crate::error::{AppError, ErrorDetail, ErrorDetailVariant, ErrorResponse};
use crate::docs::security::ApiSecurityAddon;
use crate::dto::auth::{AuthInitResponse, AuthTokenResponse, RefreshTokenRequest, TokenType, User};
use crate::handlers::{auth,oidc,chat,chat_stream,file,message,admin_users,admin_sso_provider,admin_org,admin_ai,models,admin_department};
use crate::models::messages::ChatRole;
use crate::models::users::{UserRole, UserStatus};

#[derive(OpenApi)]
#[openapi(
    paths(
        auth::handle_refresh_token,
        oidc::oidc_login_start,
        oidc::oidc_oauth_callback_get,
        oidc::oidc_oauth_callback_post,
        chat::get_chat_by_id,
        chat::get_chats,
        chat::delete_chat_by_id,
        chat::update_chat_by_id,
        chat_stream::handle_chat_stream_doc,
        chat_stream::handle_chat_stream_path_doc,
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
        admin_ai::validate_ai_engines_by_key,
        admin_ai::delete_ai_engines_api_key_key,
        admin_ai::get_ai_engine_models_by_key,
        admin_department::get_departments,
        admin_sso_provider::get_sso_providers,
        admin_sso_provider::get_sso_provider_by_id,
        admin_sso_provider::update_sso_provider_by_id,
        admin_sso_provider::delete_sso_provider_by_id,
        file::get_file_by_id,
        file::get_files,
        file::delete_file_by_id,
        file::download_file,
        file::upload_file,
        models::get_list_models,
    ),
    components(
        schemas(
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
            FileResponse,
            FileUploadRequest,
            ProviderInfo,
            ModelInfo,
            Department,
            DepartmentResponse,
            AiEngineValidationResponse,
            AiEngineModelsResponse,
            AiModel,
            AiModelCapabilities,
            SsoProviderResponse,
            SsoProviderUpdateRequest,
            AuthError,
            AppError,
            RefreshTokenRequest,
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