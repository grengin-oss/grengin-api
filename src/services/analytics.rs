use crate::{
    dto::analytics::{
        AnalyticsOverview, AnalyticsTimeSeries, DepartmentAnalytics, DepartmentAnalyticsItem,
        DepartmentAnalyticsQuery, DepartmentAnalyticsSortRule, ModelUsage,
        ScopedUserAnalyticsQuery, TimeSeriesDataPoint, UserAnalytics, UserAnalyticsItem,
        UserAnalyticsQuery,
    },
    models::{
        conversations, departments,
        messages::{self, ChatRole},
        users::{self},
    },
    services::authorization::is_path_within_scope,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use migration::{ExprTrait, SimpleExpr, extension::postgres::PgExpr};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, JoinType,
    Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait,
    sea_query::{Alias, BinOper, Expr, Func, Query},
};
use uuid::Uuid;

fn scope_condition(scope_paths: &[String]) -> Condition {
    let mut cond = Condition::any();
    for path in scope_paths {
        cond = cond.add(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(path.clone()).cast_as(Alias::new("ltree")),
        ));
    }
    cond
}

fn empty_user_analytics_response(page: u64, limit: u64) -> UserAnalytics {
    UserAnalytics {
        users: Vec::new(),
        total: 0,
        page,
        limit,
        total_pages: 0,
    }
}

fn empty_department_analytics_response(limit: u64, offset: u64) -> DepartmentAnalytics {
    DepartmentAnalytics {
        departments: Vec::new(),
        total: 0,
        limit,
        offset,
        total_pages: 0,
    }
}

#[derive(Debug, FromQueryResult)]
struct UserAnalyticsRow {
    #[sea_orm(from_alias = "user_id")]
    user_id: Uuid,
    #[sea_orm(from_alias = "user_email")]
    user_email: String,
    #[sea_orm(from_alias = "user_name")]
    user_name: Option<String>,
    #[sea_orm(from_alias = "department_id")]
    department_id: Option<Uuid>,
    #[sea_orm(from_alias = "department_name")]
    department_name: Option<String>,

    #[sea_orm(from_alias = "total_requests")]
    total_requests: i64,
    #[sea_orm(from_alias = "total_tokens")]
    total_tokens: i64,
    #[sea_orm(from_alias = "total_cost")]
    total_cost: Decimal,

    #[sea_orm(from_alias = "average_latency")]
    average_latency: Option<f64>,

    #[sea_orm(from_alias = "success_count")]
    success_count: i64,

    #[sea_orm(from_alias = "last_activity")]
    last_activity: Option<DateTime<Utc>>,
}

fn day_start_utc(d: NaiveDate) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(d.and_hms_opt(0, 0, 0).unwrap(), Utc)
}
fn day_end_utc(d: NaiveDate) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(d.and_hms_opt(23, 59, 59).unwrap(), Utc)
}

pub async fn calculate_user_analytics(
    db: &DatabaseConnection,
    query: UserAnalyticsQuery,
    page: u64,
    limit: u64,
) -> Result<UserAnalytics, DbErr> {
    // active_message = (deleted = false) AND created_at between dates (if provided)
    let mut active_msg_cond = Expr::col((messages::Entity, messages::Column::Deleted)).eq(false);

    if let Some(sd) = query.start_date {
        let from = day_start_utc(sd);
        active_msg_cond = active_msg_cond
            .and(Expr::col((messages::Entity, messages::Column::CreatedAt)).gte(from));
    }
    if let Some(ed) = query.end_date {
        let to = day_end_utc(ed);
        active_msg_cond =
            active_msg_cond.and(Expr::col((messages::Entity, messages::Column::CreatedAt)).lte(to));
    }
    // Requests = user-role messages
    let user_request_cond = active_msg_cond
        .clone()
        .and(Expr::col((messages::Entity, messages::Column::Role)).eq(messages::ChatRole::User));
    let success_count_cond = active_msg_cond.clone().and(
        Expr::col((messages::Entity, messages::Column::Role)).eq(messages::ChatRole::Assistant),
    );

    // Aggregates
    let total_requests_expr = Func::sum(Expr::case(user_request_cond, 1).finally(0));
    let success_count_expr = Func::sum(Expr::case(success_count_cond, 1).finally(0));
    let sort_email_expr = Expr::col((users::Entity, users::Column::Email));
    let sort_name_expr: SimpleExpr = Func::coalesce([
        Expr::col((users::Entity, users::Column::Name)).into(),
        Expr::val("").into(),
    ])
    .into();
    let total_tokens_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone(),
            Expr::col((messages::Entity, messages::Column::TotalTokens)),
        )
        .finally(0),
    );

    let total_cost_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone(),
            Expr::col((messages::Entity, messages::Column::Cost)),
        )
        .finally(Expr::val(Decimal::ZERO)),
    );

    // latency: CASE ... ELSE NULL (typed null), then AVG(...) and cast
    // NOTE: adjust i64 to the actual latency column type if needed (i32/f64/etc.)
    let latency_case: SimpleExpr = Expr::case(
        active_msg_cond.clone(),
        Expr::col((messages::Entity, messages::Column::Latency)),
    )
    .finally(Expr::val(None::<i32>))
    .into();

    let average_latency_expr = Func::avg(latency_case).cast_as("double precision");

    let last_activity_expr = Func::max(
        Expr::case(
            active_msg_cond.clone(),
            Expr::col((messages::Entity, messages::Column::CreatedAt)),
        )
        .finally(Expr::val(None::<DateTime<Utc>>)),
    );

    // Base query
    let mut select = users::Entity::find()
        .select_only()
        .column_as(users::Column::Id, "user_id")
        .column_as(users::Column::Email, "user_email")
        .column_as(users::Column::Name, "user_name")
        .column_as(users::Column::DepartmentId, "department_id")
        .expr_as(total_requests_expr.clone(), "total_requests")
        .expr_as(total_tokens_expr.clone(), "total_tokens")
        .expr_as(total_cost_expr.clone(), "total_cost")
        .expr_as(average_latency_expr.clone(), "average_latency")
        .expr_as(last_activity_expr.clone(), "last_activity")
        .expr_as(success_count_expr.clone(), "success_count")
        .join(JoinType::LeftJoin, users::Relation::Departments.def())
        .column_as(departments::Column::Name, "department_name")
        .join(JoinType::LeftJoin, users::Relation::Conversations.def())
        .join(JoinType::LeftJoin, conversations::Relation::Messages.def())
        .group_by(users::Column::Id)
        .group_by(users::Column::Email)
        .group_by(users::Column::Name)
        .group_by(users::Column::DepartmentId)
        .group_by(departments::Column::Name);

    // Sorting: ORDER BY THE EXPRESSION (not alias)
    if let Some(search) = &query.search {
        select = select.filter(
            Condition::any()
                .add(
                    users::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    users::Column::Email
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    departments::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                ),
        );
    }
    if query.unassigned_department.unwrap_or(false) {
        select = select.filter(users::Column::DepartmentId.is_null())
    }
    let sort_by = query.sort_by.as_deref().unwrap_or("lastActivity");
    let order = query.order.as_deref().unwrap_or("desc");
    let ord = if order.eq_ignore_ascii_case("asc") {
        Order::Asc
    } else {
        Order::Desc
    };

    select = match sort_by {
        "email" => select.order_by(sort_email_expr, ord),
        "name" => select.order_by(sort_name_expr, ord),
        "totalRequests" => select.order_by(Expr::expr(total_requests_expr.clone()), ord),
        "totalTokens" => select.order_by(Expr::expr(total_tokens_expr.clone()), ord),
        "totalCost" => select.order_by(Expr::expr(total_cost_expr.clone()), ord),
        "averageLatency" => select.order_by(average_latency_expr.clone(), ord), // this one is already SimpleExpr
        "lastActivity" | _ => select.order_by(Expr::expr(last_activity_expr.clone()), ord),
    };
    let paginator = select.into_model::<UserAnalyticsRow>().paginate(db, limit);
    let stats = paginator.num_items_and_pages().await?;

    let rows = paginator.fetch_page(page).await?;

    let users = rows
        .into_iter()
        .map(|r| UserAnalyticsItem {
            user_id: r.user_id,
            user_email: r.user_email,
            user_name: r.user_name,
            department: r.department_name,
            department_id: r.department_id,
            total_requests: r.total_requests,
            total_tokens: r.total_tokens,
            total_cost: r.total_cost.to_string().parse().unwrap_or(0.0),
            average_latency: r.average_latency.unwrap_or(0.0),
            success_count: r.success_count,
            error_count: (r.total_requests - r.success_count).max(0),
            last_activity: r.last_activity,
        })
        .collect();

    Ok(UserAnalytics {
        users,
        total: stats.number_of_items as i64,
        page,
        limit,
        total_pages: stats.number_of_pages,
    })
}

pub async fn calculate_user_analytics_scoped(
    db: &DatabaseConnection,
    query: ScopedUserAnalyticsQuery,
    page: u64,
    limit: u64,
    scope_paths: &[String],
) -> Result<UserAnalytics, DbErr> {
    if scope_paths.is_empty() {
        return Ok(empty_user_analytics_response(page, limit));
    }

    let mut active_msg_cond = Expr::col((messages::Entity, messages::Column::Deleted)).eq(false);

    if let Some(sd) = query.start_date {
        let from = day_start_utc(sd);
        active_msg_cond = active_msg_cond
            .and(Expr::col((messages::Entity, messages::Column::CreatedAt)).gte(from));
    }
    if let Some(ed) = query.end_date {
        let to = day_end_utc(ed);
        active_msg_cond =
            active_msg_cond.and(Expr::col((messages::Entity, messages::Column::CreatedAt)).lte(to));
    }

    let user_request_cond = active_msg_cond
        .clone()
        .and(Expr::col((messages::Entity, messages::Column::Role)).eq(messages::ChatRole::User));
    let success_count_cond = active_msg_cond.clone().and(
        Expr::col((messages::Entity, messages::Column::Role)).eq(messages::ChatRole::Assistant),
    );

    let total_requests_expr = Func::sum(Expr::case(user_request_cond, 1).finally(0));
    let success_count_expr = Func::sum(Expr::case(success_count_cond, 1).finally(0));
    let sort_email_expr = Expr::col((users::Entity, users::Column::Email));
    let sort_name_expr: SimpleExpr = Func::coalesce([
        Expr::col((users::Entity, users::Column::Name)).into(),
        Expr::val("").into(),
    ])
    .into();
    let total_tokens_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone(),
            Expr::col((messages::Entity, messages::Column::TotalTokens)),
        )
        .finally(0),
    );

    let total_cost_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone(),
            Expr::col((messages::Entity, messages::Column::Cost)),
        )
        .finally(Expr::val(Decimal::ZERO)),
    );

    let latency_case: SimpleExpr = Expr::case(
        active_msg_cond.clone(),
        Expr::col((messages::Entity, messages::Column::Latency)),
    )
    .finally(Expr::val(None::<i32>))
    .into();

    let average_latency_expr = Func::avg(latency_case).cast_as("double precision");

    let last_activity_expr = Func::max(
        Expr::case(
            active_msg_cond.clone(),
            Expr::col((messages::Entity, messages::Column::CreatedAt)),
        )
        .finally(Expr::val(None::<DateTime<Utc>>)),
    );

    let mut select = users::Entity::find()
        .select_only()
        .column_as(users::Column::Id, "user_id")
        .column_as(users::Column::Email, "user_email")
        .column_as(users::Column::Name, "user_name")
        .column_as(users::Column::DepartmentId, "department_id")
        .expr_as(total_requests_expr.clone(), "total_requests")
        .expr_as(total_tokens_expr.clone(), "total_tokens")
        .expr_as(total_cost_expr.clone(), "total_cost")
        .expr_as(average_latency_expr.clone(), "average_latency")
        .expr_as(last_activity_expr.clone(), "last_activity")
        .expr_as(success_count_expr.clone(), "success_count")
        .join(JoinType::LeftJoin, users::Relation::Departments.def())
        .column_as(departments::Column::Name, "department_name")
        .join(JoinType::LeftJoin, users::Relation::Conversations.def())
        .join(JoinType::LeftJoin, conversations::Relation::Messages.def())
        .group_by(users::Column::Id)
        .group_by(users::Column::Email)
        .group_by(users::Column::Name)
        .group_by(users::Column::DepartmentId)
        .group_by(departments::Column::Name);

    select = select.filter(scope_condition(scope_paths));

    if let Some(dept_id) = query.department_id {
        let dept_path = departments::Entity::find_by_id(dept_id)
            .select_only()
            .expr_as(
                Expr::col((departments::Entity, departments::Column::Path))
                    .cast_as(Alias::new("text")),
                "path",
            )
            .into_tuple::<String>()
            .one(db)
            .await?
            .unwrap_or_default();
        if dept_path.is_empty()
            || !scope_paths
                .iter()
                .any(|scope| is_path_within_scope(scope, &dept_path))
        {
            return Ok(empty_user_analytics_response(page, limit));
        }
        select = select.filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(dept_path).cast_as(Alias::new("ltree")),
        ));
    }

    if let Some(search) = &query.search {
        select = select.filter(
            Condition::any()
                .add(
                    users::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    users::Column::Email
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    departments::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                ),
        );
    }

    let sort_by = query.sort_by.as_deref().unwrap_or("lastActivity");
    let order = query.order.as_deref().unwrap_or("desc");
    let ord = if order.eq_ignore_ascii_case("asc") {
        Order::Asc
    } else {
        Order::Desc
    };

    select = match sort_by {
        "email" => select.order_by(sort_email_expr, ord),
        "name" => select.order_by(sort_name_expr, ord),
        "totalRequests" => select.order_by(Expr::expr(total_requests_expr.clone()), ord),
        "totalTokens" => select.order_by(Expr::expr(total_tokens_expr.clone()), ord),
        "totalCost" => select.order_by(Expr::expr(total_cost_expr.clone()), ord),
        "averageLatency" => select.order_by(average_latency_expr.clone(), ord),
        "lastActivity" | _ => select.order_by(Expr::expr(last_activity_expr.clone()), ord),
    };

    let paginator = select.into_model::<UserAnalyticsRow>().paginate(db, limit);
    let stats = paginator.num_items_and_pages().await?;

    let rows = paginator.fetch_page(page).await?;

    let users = rows
        .into_iter()
        .map(|r| UserAnalyticsItem {
            user_id: r.user_id,
            user_email: r.user_email,
            user_name: r.user_name,
            department: r.department_name,
            department_id: r.department_id,
            total_requests: r.total_requests,
            total_tokens: r.total_tokens,
            total_cost: r.total_cost.to_string().parse().unwrap_or(0.0),
            average_latency: r.average_latency.unwrap_or(0.0),
            success_count: r.success_count,
            error_count: (r.total_requests - r.success_count).max(0),
            last_activity: r.last_activity,
        })
        .collect();

    Ok(UserAnalytics {
        users,
        total: stats.number_of_items as i64,
        page,
        limit,
        total_pages: stats.number_of_pages,
    })
}

#[derive(Debug, FromQueryResult)]
struct DepartmentAnalyticsRow {
    #[sea_orm(from_alias = "department")]
    department: String,

    #[sea_orm(from_alias = "total_users")]
    total_users: Option<i64>,

    #[sea_orm(from_alias = "total_requests")]
    total_requests: Option<i64>,

    #[sea_orm(from_alias = "total_tokens")]
    total_tokens: Option<i64>,

    #[sea_orm(from_alias = "total_cost")]
    total_cost: Option<Decimal>,

    #[sea_orm(from_alias = "average_latency")]
    average_latency: Option<f64>,

    #[sea_orm(from_alias = "success_count")]
    success_count: Option<i64>,
}

pub async fn get_department_analytics(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    limit: Option<u64>,
    offset: Option<u64>,
    search: Option<String>,
    sort: Option<DepartmentAnalyticsSortRule>,
    ascending: Option<bool>,
) -> Result<DepartmentAnalytics, DbErr> {
    let mut limit = limit.unwrap_or(20);
    if limit == 0 {
        limit = 20;
    }
    let offset = offset.unwrap_or(0);
    // Message window condition (kept inside CASE so departments with 0 messages still show up)
    let mut active_msg_cond = Expr::col((messages::Entity, messages::Column::Deleted)).eq(false);

    if let Some(sd) = start_date {
        active_msg_cond = active_msg_cond
            .and(Expr::col((messages::Entity, messages::Column::CreatedAt)).gte(day_start_utc(sd)));
    }
    if let Some(ed) = end_date {
        active_msg_cond = active_msg_cond
            .and(Expr::col((messages::Entity, messages::Column::CreatedAt)).lte(day_end_utc(ed)));
    }

    // Exclude deleted users from *all* metrics
    let active_user_cond =
        Expr::col((users::Entity, users::Column::Status)).ne(users::UserStatus::Deleted);

    // Requests / success / error
    let req_cond = active_msg_cond
        .clone()
        .and(active_user_cond.clone())
        .and(Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::User));

    let success_cond = active_msg_cond
        .clone()
        .and(active_user_cond.clone())
        .and(Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::Assistant));

    let total_requests_expr = Func::sum(Expr::case(req_cond, 1).finally(0));
    let success_count_expr = Func::sum(Expr::case(success_cond, 1).finally(0));

    // Tokens + cost (only for active window + active users)
    let total_tokens_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone().and(active_user_cond.clone()),
            Expr::col((messages::Entity, messages::Column::TotalTokens)),
        )
        .finally(0),
    );

    let total_cost_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone().and(active_user_cond.clone()),
            Expr::col((messages::Entity, messages::Column::Cost)),
        )
        .finally(Expr::val(Decimal::ZERO)),
    );

    // Avg latency (NULL-safe) in the same window
    let latency_case: SimpleExpr = Expr::case(
        active_msg_cond
            .clone()
            .and(active_user_cond.clone())
            .and(Expr::col((messages::Entity, messages::Column::Latency)).is_not_null()),
        Expr::col((messages::Entity, messages::Column::Latency)),
    )
    .finally(Expr::val(None::<i32>))
    .into();

    let average_latency_expr = Func::avg(latency_case).cast_as("double precision");

    // Total users per department: COUNT(DISTINCT CASE WHEN user.status <> deleted THEN user.id ELSE NULL END)
    let user_id_case: SimpleExpr = Expr::case(
        active_user_cond.clone(),
        Expr::col((users::Entity, users::Column::Id)),
    )
    .finally(Expr::val(None::<Uuid>))
    .into();

    let total_users_expr = Func::count_distinct(user_id_case);

    // Build query: departments -> users -> conversations -> messages
    let child_alias = Alias::new("child_dept");
    let child_subquery = Query::select()
        .expr(Func::count(Expr::col((
            child_alias.clone(),
            departments::Column::Id,
        ))))
        .from_as(departments::Entity, child_alias.clone())
        .and_where(
            Expr::col((child_alias.clone(), departments::Column::ParentId))
                .eq(Expr::col((departments::Entity, departments::Column::Id))),
        )
        .to_owned();
    let child_count_expr: SimpleExpr =
        SimpleExpr::SubQuery(None, Box::new(child_subquery.into_sub_query_statement()));

    let mut select = departments::Entity::find()
        .select_only()
        .column_as(departments::Column::Name, "department")
        .expr_as(total_users_expr.clone(), "total_users")
        .expr_as(total_requests_expr, "total_requests")
        .expr_as(total_tokens_expr, "total_tokens")
        .expr_as(total_cost_expr, "total_cost")
        .expr_as(average_latency_expr, "average_latency")
        .expr_as(success_count_expr, "success_count")
        .expr_as(child_count_expr.clone(), "sub_departments")
        .join(JoinType::LeftJoin, departments::Relation::Users.def())
        .join(JoinType::LeftJoin, users::Relation::Conversations.def())
        .join(JoinType::LeftJoin, conversations::Relation::Messages.def())
        .group_by(departments::Column::Id)
        .group_by(departments::Column::Name);

    let sort_rule = sort.unwrap_or(DepartmentAnalyticsSortRule::Name);
    let order = if ascending.unwrap_or(false) {
        Order::Asc
    } else {
        Order::Desc
    };
    select = match sort_rule {
        DepartmentAnalyticsSortRule::Name => select.order_by(departments::Column::Name, order),
        DepartmentAnalyticsSortRule::CreatedAt => {
            select.order_by(departments::Column::CreatedAt, order)
        }
        DepartmentAnalyticsSortRule::UpdatedAt => {
            select.order_by(departments::Column::UpdatedAt, order)
        }
        DepartmentAnalyticsSortRule::Members => {
            select.order_by(Expr::expr(total_users_expr.clone()), order)
        }
        DepartmentAnalyticsSortRule::SubDepartments => {
            select.order_by(Expr::expr(child_count_expr.clone()), order)
        }
    };

    if let Some(search) = search.as_ref().filter(|s| !s.trim().is_empty()) {
        select = select.filter(
            departments::Column::Name
                .into_expr()
                .ilike(format!("%{}%", search)),
        );
    }

    let paginator = select
        .into_model::<DepartmentAnalyticsRow>()
        .paginate(db, limit);
    let page = offset / limit;
    let stats = paginator.num_items_and_pages().await?;
    let rows = paginator.fetch_page(page).await?;

    let analytics = rows
        .into_iter()
        .map(|r| {
            let total_cost = r
                .total_cost
                .unwrap_or(Decimal::ZERO)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let total_requests = r.total_requests.unwrap_or(0);
            let success_count = r.success_count.unwrap_or(0);

            DepartmentAnalyticsItem {
                department: r.department,
                total_users: r.total_users.unwrap_or(0),
                total_requests,
                total_tokens: r.total_tokens.unwrap_or(0),
                total_cost,
                average_latency: r.average_latency.unwrap_or(0.0),
                success_count,
                error_count: (total_requests - success_count).max(0),
            }
        })
        .collect::<Vec<_>>();

    Ok(DepartmentAnalytics {
        total: stats.number_of_items as i64,
        limit,
        offset,
        total_pages: stats.number_of_pages,
        departments: analytics,
    })
}

pub async fn get_department_analytics_scoped(
    db: &DatabaseConnection,
    query: DepartmentAnalyticsQuery,
    scope_paths: &[String],
) -> Result<DepartmentAnalytics, DbErr> {
    let mut limit = query.limit.unwrap_or(20);
    if limit == 0 {
        limit = 20;
    }
    let offset = query.offset.unwrap_or(0);

    if scope_paths.is_empty() {
        return Ok(empty_department_analytics_response(limit, offset));
    }

    let mut active_msg_cond = Expr::col((messages::Entity, messages::Column::Deleted)).eq(false);

    if let Some(sd) = query.start_date {
        active_msg_cond = active_msg_cond
            .and(Expr::col((messages::Entity, messages::Column::CreatedAt)).gte(day_start_utc(sd)));
    }
    if let Some(ed) = query.end_date {
        active_msg_cond = active_msg_cond
            .and(Expr::col((messages::Entity, messages::Column::CreatedAt)).lte(day_end_utc(ed)));
    }

    let active_user_cond =
        Expr::col((users::Entity, users::Column::Status)).ne(users::UserStatus::Deleted);

    let req_cond = active_msg_cond
        .clone()
        .and(active_user_cond.clone())
        .and(Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::User));

    let success_cond = active_msg_cond
        .clone()
        .and(active_user_cond.clone())
        .and(Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::Assistant));

    let total_requests_expr = Func::sum(Expr::case(req_cond, 1).finally(0));
    let success_count_expr = Func::sum(Expr::case(success_cond, 1).finally(0));

    let total_tokens_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone().and(active_user_cond.clone()),
            Expr::col((messages::Entity, messages::Column::TotalTokens)),
        )
        .finally(0),
    );

    let total_cost_expr = Func::sum(
        Expr::case(
            active_msg_cond.clone().and(active_user_cond.clone()),
            Expr::col((messages::Entity, messages::Column::Cost)),
        )
        .finally(Expr::val(Decimal::ZERO)),
    );

    let latency_case: SimpleExpr = Expr::case(
        active_msg_cond
            .clone()
            .and(active_user_cond.clone())
            .and(Expr::col((messages::Entity, messages::Column::Latency)).is_not_null()),
        Expr::col((messages::Entity, messages::Column::Latency)),
    )
    .finally(Expr::val(None::<i32>))
    .into();

    let average_latency_expr = Func::avg(latency_case).cast_as("double precision");

    let user_id_case: SimpleExpr = Expr::case(
        active_user_cond.clone(),
        Expr::col((users::Entity, users::Column::Id)),
    )
    .finally(Expr::val(None::<Uuid>))
    .into();

    let total_users_expr = Func::count_distinct(user_id_case);

    let child_alias = Alias::new("child_dept");
    let child_subquery = Query::select()
        .expr(Func::count(Expr::col((
            child_alias.clone(),
            departments::Column::Id,
        ))))
        .from_as(departments::Entity, child_alias.clone())
        .and_where(
            Expr::col((child_alias.clone(), departments::Column::ParentId))
                .eq(Expr::col((departments::Entity, departments::Column::Id))),
        )
        .to_owned();
    let child_count_expr: SimpleExpr =
        SimpleExpr::SubQuery(None, Box::new(child_subquery.into_sub_query_statement()));

    let mut select = departments::Entity::find()
        .select_only()
        .column_as(departments::Column::Name, "department")
        .expr_as(total_users_expr.clone(), "total_users")
        .expr_as(total_requests_expr, "total_requests")
        .expr_as(total_tokens_expr, "total_tokens")
        .expr_as(total_cost_expr, "total_cost")
        .expr_as(average_latency_expr, "average_latency")
        .expr_as(success_count_expr, "success_count")
        .expr_as(child_count_expr.clone(), "sub_departments")
        .join(JoinType::LeftJoin, departments::Relation::Users.def())
        .join(JoinType::LeftJoin, users::Relation::Conversations.def())
        .join(JoinType::LeftJoin, conversations::Relation::Messages.def())
        .group_by(departments::Column::Id)
        .group_by(departments::Column::Name);

    let sort_rule = query.sort.unwrap_or(DepartmentAnalyticsSortRule::Name);
    let order = if query.ascending.unwrap_or(false) {
        Order::Asc
    } else {
        Order::Desc
    };
    select = match sort_rule {
        DepartmentAnalyticsSortRule::Name => select.order_by(departments::Column::Name, order),
        DepartmentAnalyticsSortRule::CreatedAt => {
            select.order_by(departments::Column::CreatedAt, order)
        }
        DepartmentAnalyticsSortRule::UpdatedAt => {
            select.order_by(departments::Column::UpdatedAt, order)
        }
        DepartmentAnalyticsSortRule::Members => {
            select.order_by(Expr::expr(total_users_expr.clone()), order)
        }
        DepartmentAnalyticsSortRule::SubDepartments => {
            select.order_by(Expr::expr(child_count_expr.clone()), order)
        }
    };

    select = select.filter(scope_condition(scope_paths));

    if let Some(dept_id) = query.department_id {
        let dept_path = departments::Entity::find_by_id(dept_id)
            .select_only()
            .expr_as(
                Expr::col((departments::Entity, departments::Column::Path))
                    .cast_as(Alias::new("text")),
                "path",
            )
            .into_tuple::<String>()
            .one(db)
            .await?
            .unwrap_or_default();
        if dept_path.is_empty()
            || !scope_paths
                .iter()
                .any(|scope| is_path_within_scope(scope, &dept_path))
        {
            return Ok(empty_department_analytics_response(limit, offset));
        }
        select = select.filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(dept_path).cast_as(Alias::new("ltree")),
        ));
    }

    if let Some(search) = query.search.as_ref().filter(|s| !s.trim().is_empty()) {
        select = select.filter(
            departments::Column::Name
                .into_expr()
                .ilike(format!("%{}%", search)),
        );
    }

    let paginator = select
        .into_model::<DepartmentAnalyticsRow>()
        .paginate(db, limit);
    let page = offset / limit;
    let stats = paginator.num_items_and_pages().await?;
    let rows = paginator.fetch_page(page).await?;

    let analytics = rows
        .into_iter()
        .map(|r| {
            let total_cost = r
                .total_cost
                .unwrap_or(Decimal::ZERO)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let total_requests = r.total_requests.unwrap_or(0);
            let success_count = r.success_count.unwrap_or(0);

            DepartmentAnalyticsItem {
                department: r.department,
                total_users: r.total_users.unwrap_or(0),
                total_requests,
                total_tokens: r.total_tokens.unwrap_or(0),
                total_cost,
                average_latency: r.average_latency.unwrap_or(0.0),
                success_count,
                error_count: (total_requests - success_count).max(0),
            }
        })
        .collect::<Vec<_>>();

    Ok(DepartmentAnalytics {
        total: stats.number_of_items as i64,
        limit,
        offset,
        total_pages: stats.number_of_pages,
        departments: analytics,
    })
}

fn pct_growth(current: i64, prev: i64) -> f64 {
    if prev == 0 {
        return if current == 0 { 0.0 } else { 100.0 };
    }
    ((current - prev) as f64 / prev as f64) * 100.0
}

#[derive(Debug, FromQueryResult)]
struct OverviewAggRow {
    #[sea_orm(from_alias = "total_requests")]
    total_requests: Option<i64>,
    #[sea_orm(from_alias = "total_tokens")]
    total_tokens: Option<i64>,
    #[sea_orm(from_alias = "total_cost")]
    total_cost: Option<Decimal>,
}

#[derive(Debug, FromQueryResult)]
struct ActiveUsersRow {
    #[sea_orm(from_alias = "active_users")]
    active_users: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct TopModelRow {
    #[sea_orm(from_alias = "model_provider")]
    model_provider: String,
    #[sea_orm(from_alias = "model_name")]
    model_name: String,
    #[sea_orm(from_alias = "total_requests")]
    total_requests: i64,
    #[sea_orm(from_alias = "total_tokens")]
    total_tokens: i64,
    #[sea_orm(from_alias = "total_cost")]
    total_cost: Decimal,
}

fn build_msg_filter(start: Option<NaiveDate>, end: Option<NaiveDate>) -> sea_orm::Condition {
    let mut cond = sea_orm::Condition::all()
        .add(Expr::col((messages::Entity, messages::Column::Deleted)).eq(false));

    if let Some(sd) = start {
        cond = cond
            .add(Expr::col((messages::Entity, messages::Column::CreatedAt)).gte(day_start_utc(sd)));
    }
    if let Some(ed) = end {
        cond = cond
            .add(Expr::col((messages::Entity, messages::Column::CreatedAt)).lte(day_end_utc(ed)));
    }
    cond
}

pub async fn get_overview_analytics(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<AnalyticsOverview, DbErr> {
    // total users (all rows)
    let total_users = users::Entity::find().count(db).await? as i64;

    // current window: total_requests / total_tokens / total_cost from messages
    let cur_cond = build_msg_filter(start_date, end_date);

    let total_requests_expr = Func::sum(
        Expr::case(
            Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::User),
            1,
        )
        .finally(0),
    );

    let total_tokens_expr = Func::sum(Expr::col((messages::Entity, messages::Column::TotalTokens)));
    let total_cost_expr = Func::sum(Expr::col((messages::Entity, messages::Column::Cost)));

    let cur_q = messages::Entity::find()
        .select_only()
        .expr_as(total_requests_expr.clone(), "total_requests")
        .expr_as(total_tokens_expr.clone(), "total_tokens")
        .expr_as(total_cost_expr.clone(), "total_cost")
        .filter(cur_cond);

    let cur_row = cur_q
        .into_model::<OverviewAggRow>()
        .one(db)
        .await?
        .unwrap_or(OverviewAggRow {
            total_requests: Some(0),
            total_tokens: Some(0),
            total_cost: Some(Decimal::ZERO),
        });

    let total_requests = cur_row.total_requests.unwrap_or(0);
    let total_tokens = cur_row.total_tokens.unwrap_or(0);
    let total_cost_dec = cur_row.total_cost.unwrap_or(Decimal::ZERO);
    let total_cost = total_cost_dec.to_string().parse().unwrap_or(0.0);

    // active users (distinct user_id having at least one USER message in window)
    let active_users_cond = build_msg_filter(start_date, end_date)
        .add(Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::User));

    let active_users_q = conversations::Entity::find()
        .select_only()
        .join(
            sea_orm::JoinType::InnerJoin,
            conversations::Relation::Messages.def(),
        )
        .filter(active_users_cond)
        .expr_as(
            Func::count_distinct(Expr::col((
                conversations::Entity,
                conversations::Column::UserId,
            ))),
            "active_users",
        );

    let active_users = active_users_q
        .into_model::<ActiveUsersRow>()
        .one(db)
        .await?
        .and_then(|r| r.active_users)
        .unwrap_or(0);

    let average_requests_per_user = if active_users > 0 {
        total_requests as f64 / active_users as f64
    } else {
        0.0
    };

    // top models (group by provider/name)
    // requests = user-role messages; tokens/cost = all messages (still within window)
    let top_models_q = messages::Entity::find()
        .select_only()
        .column_as(messages::Column::ModelProvider, "model_provider")
        .column_as(messages::Column::ModelName, "model_name")
        .expr_as(
            Func::sum(
                Expr::case(
                    Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::User),
                    1,
                )
                .finally(0),
            ),
            "total_requests",
        )
        .expr_as(
            Func::sum(Expr::col((messages::Entity, messages::Column::TotalTokens))),
            "total_tokens",
        )
        .expr_as(
            Func::sum(Expr::col((messages::Entity, messages::Column::Cost))),
            "total_cost",
        )
        .filter(build_msg_filter(start_date, end_date))
        .group_by(messages::Column::ModelProvider)
        .group_by(messages::Column::ModelName)
        .order_by_desc(Expr::expr(Func::sum(Expr::col((
            messages::Entity,
            messages::Column::Cost,
        )))))
        .limit(10);

    let top_model_rows = top_models_q.into_model::<TopModelRow>().all(db).await?;
    let top_models = top_model_rows
        .into_iter()
        .map(|r| ModelUsage {
            model_provider: r.model_provider,
            model_name: r.model_name,
            total_requests: r.total_requests,
            total_tokens: r.total_tokens,
            total_cost: r.total_cost.to_string().parse().unwrap_or(0.0),
        })
        .collect::<Vec<_>>();

    // growth rates (only if BOTH dates provided)
    let (request_growth_rate, token_growth_rate, cost_growth_rate) =
        if let (Some(sd), Some(ed)) = (start_date, end_date) {
            let days = (ed - sd).num_days() + 1; // inclusive window
            let prev_end = sd - Duration::days(1);
            let prev_start = prev_end - Duration::days(days - 1);

            let prev_cond = build_msg_filter(Some(prev_start), Some(prev_end));

            let prev_q = messages::Entity::find()
                .select_only()
                .expr_as(total_requests_expr, "total_requests")
                .expr_as(total_tokens_expr, "total_tokens")
                .expr_as(total_cost_expr, "total_cost")
                .filter(prev_cond);

            let prev_row = prev_q
                .into_model::<OverviewAggRow>()
                .one(db)
                .await?
                .unwrap_or(OverviewAggRow {
                    total_requests: Some(0),
                    total_tokens: Some(0),
                    total_cost: Some(Decimal::ZERO),
                });

            let prev_requests = prev_row.total_requests.unwrap_or(0);
            let prev_tokens = prev_row.total_tokens.unwrap_or(0);
            let prev_cost = prev_row
                .total_cost
                .unwrap_or(Decimal::ZERO)
                .to_string()
                .parse()
                .unwrap_or(0.0);

            (
                pct_growth(total_requests, prev_requests),
                pct_growth(total_tokens, prev_tokens),
                if prev_cost == 0.0 {
                    if total_cost == 0.0 { 0.0 } else { 100.0 }
                } else {
                    ((total_cost - prev_cost) / prev_cost) * 100.0
                },
            )
        } else {
            // no date range -> no growth calc
            (0.0, 0.0, 0.0)
        };

    Ok(AnalyticsOverview {
        total_users,
        active_users,
        total_requests,
        total_tokens,
        total_cost,
        average_requests_per_user,
        top_models,
        request_growth_rate,
        token_growth_rate,
        cost_growth_rate,
    })
}

fn normalize_granularity(g: &str) -> &str {
    match g {
        "hour" | "day" | "week" | "month" => g,
        _ => "day",
    }
}

#[derive(Debug, FromQueryResult)]
struct TimeSeriesRow {
    #[sea_orm(from_alias = "bucket")]
    bucket: DateTime<Utc>,

    #[sea_orm(from_alias = "total_requests")]
    total_requests: Option<i64>,
    #[sea_orm(from_alias = "total_tokens")]
    total_tokens: Option<i64>,
    #[sea_orm(from_alias = "total_cost")]
    total_cost: Option<Decimal>,

    #[sea_orm(from_alias = "average_latency")]
    average_latency: Option<f64>,

    #[sea_orm(from_alias = "success_count")]
    success_count: Option<i64>,
}

pub async fn get_timeseries_analytics(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    granularity: String,
) -> Result<AnalyticsTimeSeries, DbErr> {
    // Filter used in WHERE (this endpoint is timeseries, so it’s fine to filter rows directly)
    let mut cond = sea_orm::Condition::all()
        .add(Expr::col((messages::Entity, messages::Column::Deleted)).eq(false));

    if let Some(sd) = start_date {
        cond = cond
            .add(Expr::col((messages::Entity, messages::Column::CreatedAt)).gte(day_start_utc(sd)));
    }
    if let Some(ed) = end_date {
        cond = cond
            .add(Expr::col((messages::Entity, messages::Column::CreatedAt)).lte(day_end_utc(ed)));
    }

    // bucket = date_trunc('day'|'hour'|'week'|'month', created_at)
    // SeaQuery builder via cust_with_exprs, still no raw SQL string query.
    let gran = normalize_granularity(&granularity); // "hour"|"day"|"week"|"month"

    // date_trunc('day', messages.created_at)
    // Use a literal granularity here so GROUP BY matches SELECT exactly (avoids param mismatch).
    let bucket_expr: SimpleExpr = Expr::cust(format!(
        "date_trunc('{}', \"messages\".\"createdAt\")",
        gran
    ));

    // total_requests = SUM(CASE WHEN role='user' THEN 1 ELSE 0 END)
    let total_requests_expr = Func::sum(
        Expr::case(
            Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::User),
            1,
        )
        .finally(0),
    );

    let total_tokens_expr = Func::sum(Expr::col((messages::Entity, messages::Column::TotalTokens)));
    let total_cost_expr = Func::sum(Expr::col((messages::Entity, messages::Column::Cost)));

    // avg latency: AVG(CASE WHEN latency IS NOT NULL THEN latency ELSE NULL END)::double precision
    // (Also respects deleted/date filter because we’re filtering in WHERE already)
    let latency_case: SimpleExpr = Expr::case(
        Expr::col((messages::Entity, messages::Column::Latency)).is_not_null(),
        Expr::col((messages::Entity, messages::Column::Latency)),
    )
    .finally(Expr::val(None::<i64>)) // adjust type if your latency column isn't i64
    .into();

    let average_latency_expr = Func::avg(latency_case).cast_as("double precision");

    // success_count = assistant messages (placeholder definition)
    let success_count_expr = Func::sum(
        Expr::case(
            Expr::col((messages::Entity, messages::Column::Role)).eq(ChatRole::Assistant),
            1,
        )
        .finally(0),
    );

    let q = messages::Entity::find()
        .select_only()
        .expr_as(bucket_expr.clone(), "bucket")
        .expr_as(total_requests_expr.clone(), "total_requests")
        .expr_as(total_tokens_expr.clone(), "total_tokens")
        .expr_as(total_cost_expr.clone(), "total_cost")
        .expr_as(average_latency_expr.clone(), "average_latency")
        .expr_as(success_count_expr.clone(), "success_count")
        .filter(cond)
        .group_by(bucket_expr.clone())
        .order_by(bucket_expr.clone(), Order::Asc);

    let rows = q.into_model::<TimeSeriesRow>().all(db).await?;

    let data = rows
        .into_iter()
        .map(|r| {
            let total_requests = r.total_requests.unwrap_or(0);
            let success_count = r.success_count.unwrap_or(0);
            TimeSeriesDataPoint {
                timestamp: r.bucket.to_rfc3339(),
                total_requests,
                total_tokens: r.total_tokens.unwrap_or(0),
                total_cost: r
                    .total_cost
                    .unwrap_or(Decimal::ZERO)
                    .to_string()
                    .parse()
                    .unwrap_or(0.0),
                average_latency: r.average_latency.unwrap_or(0.0),
                success_count,
                error_count: (total_requests - success_count).max(0),
            }
        })
        .collect();

    Ok(AnalyticsTimeSeries {
        data,
        granularity: gran.to_string(),
    })
}
