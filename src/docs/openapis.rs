use utoipa::OpenApi;
use crate::auth::claims::Claims;
use crate::auth::error::{AuthError,AuthErrorCode,AuthErrorDetailVariant,AuthErrorResponse};
use crate::dto::admin_ai::{AiEngineResponse, AiEngineUpdateRequest, AiEngineValidationResponse, AiModel,AiEngineModelsResponse, AiModelCapabilities};
use crate::dto::admin_department::{DepartmentListQuery, DepartmentRequest, DepartmentResponse, DepartmentsListResponse};
use crate::dto::branding::{BrandingResponse, BrandingUpdate};
use crate::dto::admin_sso_providers::{SsoProviderResponse, SsoProviderUpdateRequest};
use crate::dto::admin_user::{UserDetails, UserPatchRequest, UserRequest, UserResponse, UserUpdateRequest};
use crate::dto::chat::{ArchiveChatRequest, ConversationResponse, MessageParts, MessageResponse, TokenUsage};
use crate::dto::chat_stream::{ChatInitRequest, ChatStream};
use crate::dto::common::{PaginationQuery, SortRule};
use crate::dto::files::{Attachment, File, FileResponse, FileUploadRequest};
use crate::dto::models::{ModelInfo, ProviderInfo};
use crate::dto::oauth::OAuthCallback;
use crate::error::{AppError, ErrorDetail, ErrorDetailVariant, ErrorResponse};
use crate::docs::{security::ApiSecurityAddon,app_error_catlog::AppErrorCatalogItem};
use crate::dto::auth::{AuthInitResponse, AuthTokenResponse, RefreshTokenRequest, TokenType, User};
use crate::handlers::{auth,admin_department,admin_analytics,oidc,open_error,chat,chat_stream,file,message,admin_users,admin_sso_provider,branding,admin_ai,models};
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
        branding::get_branding,
        branding::get_admin_branding,
        branding::update_branding,
        admin_ai::get_ai_engines,
        admin_ai::update_ai_engines_by_key,
        admin_ai::get_ai_engines_by_key,
        admin_ai::validate_ai_engines_by_key,
        admin_ai::delete_ai_engines_api_key_key,
        admin_ai::get_ai_engine_models_by_key,
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
        open_error::get_app_error_catalog,
        open_error::get_auth_error_catalog,
        admin_analytics::get_analytics_overview,
        admin_analytics::get_user_analytics,
        admin_analytics::get_timeseries_analytics,
        admin_department::create_department,
        admin_department::update_department,
        admin_department::delete_department,
        admin_department::get_department_by_id,
        admin_department::list_departments,
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
            BrandingResponse,
            BrandingUpdate,
            UserDetails,
            UserPatchRequest,
            AiEngineResponse,
            AiEngineUpdateRequest,
            FileResponse,
            FileUploadRequest,
            ProviderInfo,
            ModelInfo,
            AiEngineValidationResponse,
            AiEngineModelsResponse,
            AiModel,
            AiModelCapabilities,
            SsoProviderResponse,
            SsoProviderUpdateRequest,
            AuthError,
            AppError,
            AuthErrorCode,
            AuthErrorDetailVariant,
            AuthErrorResponse,
            RefreshTokenRequest,
            AppErrorCatalogItem,
            DepartmentListQuery,
            DepartmentResponse,
            DepartmentsListResponse,
            DepartmentRequest,
        )
    ),
    tags(
        (name = "auth", description = "Authentication & user endpoints"),
        (name = "branding", description = "Branding configuration endpoints"),
        (name = "admin", description = "Admin endpoints"),
        (name = "root", description = "Root / health"),
    ),
    modifiers(
        &ApiSecurityAddon
    )
)]
pub struct ApiDoc;