use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use migration::OnConflict;
use std::{collections::hash_map::DefaultHasher, hash::{Hash, Hasher}};
use sea_orm::{ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde::{de::DeserializeOwned, Serialize};
use uuid::Uuid;
use crate::{
    dto::analytics::{
        DepartmentAnalyticsQuery, DepartmentAnalyticsResponse, OverviewResponse, TimeSeriesQuery,
        TimeSeriesResponse, UserAnalyticsQuery, UserAnalyticsResponse,
    },
    models::analytics,
    services::analytics as analytics_service,
};

const CACHE_WINDOWS_DAYS: [i64; 3] = [7, 30, 90];
const CACHE_WINDOW_MTD: &str = "mtd";
const CACHE_CATEGORY_OVERVIEW: &str = "overview";
const CACHE_CATEGORY_USERS: &str = "users";
const CACHE_CATEGORY_DEPARTMENTS: &str = "departments";
const CACHE_CATEGORY_TIMESERIES: &str = "timeseries";

#[derive(Clone)]
struct CacheWindow {
    key: String,
    start_date: NaiveDate,
    end_date: NaiveDate,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
}

fn day_start_utc(d: NaiveDate) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(d.and_hms_opt(0, 0, 0).unwrap(), Utc)
}

fn day_end_utc(d: NaiveDate) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(d.and_hms_opt(23, 59, 59).unwrap(), Utc)
}

fn cache_window_for_days(days: i64, today: NaiveDate) -> CacheWindow {
    let start_date = today - Duration::days(days - 1);
    let end_date = today;
    CacheWindow {
        key: format!("{days}d"),
        start_date,
        end_date,
        range_start: day_start_utc(start_date),
        range_end: day_end_utc(end_date),
    }
}

fn cache_window_for_mtd(today: NaiveDate) -> CacheWindow {
    let start_date = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
        .unwrap_or(today);
    CacheWindow {
        key: CACHE_WINDOW_MTD.to_string(),
        start_date,
        end_date: today,
        range_start: day_start_utc(start_date),
        range_end: day_end_utc(today),
    }
}

fn cache_window_for_query(start: Option<NaiveDate>, end: Option<NaiveDate>) -> Option<CacheWindow> {
    let start_date = start?;
    let end_date = end?;
    let today = Utc::now().date_naive();
    if end_date != today {
        return None;
    }

    let days = (end_date - start_date).num_days() + 1;
    if CACHE_WINDOWS_DAYS.contains(&days) {
        return Some(CacheWindow {
            key: format!("{days}d"),
            start_date,
            end_date,
            range_start: day_start_utc(start_date),
            range_end: day_end_utc(end_date),
        });
    }

    let mtd = cache_window_for_mtd(today);
    if start_date == mtd.start_date {
        return Some(mtd);
    }

    None
}

fn cache_key(category: &str, window_key: &str, suffix: &str) -> String {
    let base = format!("analytics:{category}:{window_key}");
    if suffix.is_empty() {
        return base;
    }

    let key = format!("{base}:{suffix}");
    if key.len() <= 200 {
        return key;
    }

    let mut hasher = DefaultHasher::new();
    suffix.hash(&mut hasher);
    let suffix_hash = hasher.finish();
    format!("{base}:hash={suffix_hash:x}")
}

fn json_key<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn user_cache_suffix(query: &UserAnalyticsQuery, page: u64, limit: u64) -> String {
    let sort_by = query.sort_by.as_deref().unwrap_or("lastActivity");
    let order = query.order.as_deref().unwrap_or("desc");
    let search = json_key(&query.search);
    let role = json_key(&query.role);
    let status = json_key(&query.status);
    let unassigned_department = query.unassigned_department.unwrap_or(false);

    format!(
        "page={page}&limit={limit}&sort_by={sort_by}&order={order}&search={search}&role={role}&status={status}&unassigned={unassigned_department}"
    )
}

fn department_cache_suffix(query: &DepartmentAnalyticsQuery, limit: u64, offset: u64) -> String {
    let search = json_key(&query.search);
    format!("limit={limit}&offset={offset}&search={search}")
}

fn timeseries_cache_suffix(query: &TimeSeriesQuery, granularity: &str) -> String {
    let group_by = json_key(&query.group_by);
    format!("granularity={granularity}&group_by={group_by}")
}

async fn load_cached_response<T: DeserializeOwned>(
    db: &DatabaseConnection,
    cache_key: &str,
    expected_end: Option<NaiveDate>,
) -> Result<Option<T>, DbErr> {
    let record = analytics::Entity::find()
        .filter(analytics::Column::CacheKey.eq(cache_key))
        .one(db)
        .await?;

    let Some(record) = record else {
        return Ok(None);
    };

    if let Some(expected_end) = expected_end {
        if record.range_end.date_naive() != expected_end {
            return Ok(None);
        }
    }

    match serde_json::from_value(record.payload) {
        Ok(payload) => Ok(Some(payload)),
        Err(err) => {
            eprintln!("Analytics cache deserialize error for {cache_key}: {err}");
            Ok(None)
        }
    }
}

async fn save_cached_response<T: Serialize>(
    db: &DatabaseConnection,
    cache_key: &str,
    category: &str,
    window: &CacheWindow,
    payload: &T,
) -> Result<(), DbErr> {
    let payload = serde_json::to_value(payload)
        .map_err(|err| DbErr::Custom(format!("Analytics cache serialize error: {err}")))?;
    let now = Utc::now();

    let active_model = analytics::ActiveModel {
        id: Set(Uuid::new_v4()),
        cache_key: Set(cache_key.to_string()),
        category: Set(category.to_string()),
        range_start: Set(window.range_start),
        range_end: Set(window.range_end),
        payload: Set(payload),
        created_at: Set(now),
        updated_at: Set(now),
    };

    analytics::Entity::insert(active_model)
        .on_conflict(
            OnConflict::column(analytics::Column::CacheKey)
                .update_columns([
                    analytics::Column::Category,
                    analytics::Column::RangeStart,
                    analytics::Column::RangeEnd,
                    analytics::Column::Payload,
                    analytics::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;

    Ok(())
}

pub async fn get_overview_cached(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    live: bool,
) -> Result<OverviewResponse, DbErr> {
    if let Some(window) = cache_window_for_query(start_date, end_date) {
        let key = cache_key(CACHE_CATEGORY_OVERVIEW, &window.key, "");
        if !live {
            if let Some(cached) = load_cached_response(db, &key, Some(window.end_date)).await? {
                return Ok(cached);
            }
        }

        let result = analytics_service::get_overview_analytics(
            db,
            Some(window.start_date),
            Some(window.end_date),
        )
        .await?;
        save_cached_response(db, &key, CACHE_CATEGORY_OVERVIEW, &window, &result).await?;
        return Ok(result);
    }

    analytics_service::get_overview_analytics(db, start_date, end_date).await
}

pub async fn get_user_analytics_cached(
    db: &DatabaseConnection,
    query: UserAnalyticsQuery,
) -> Result<UserAnalyticsResponse, DbErr> {
    let page = query.page.unwrap_or(0);
    let limit = query.limit.unwrap_or(20);
    let live = query.live.unwrap_or(false);

    if let Some(window) = cache_window_for_query(query.start_date, query.end_date) {
        let suffix = user_cache_suffix(&query, page, limit);
        let key = cache_key(CACHE_CATEGORY_USERS, &window.key, &suffix);
        if !live {
            if let Some(cached) = load_cached_response(db, &key, Some(window.end_date)).await? {
                return Ok(cached);
            }
        }

        let result =
            analytics_service::calculate_user_analytics(db, query, page, limit).await?;
        save_cached_response(db, &key, CACHE_CATEGORY_USERS, &window, &result).await?;
        return Ok(result);
    }

    analytics_service::calculate_user_analytics(db, query, page, limit).await
}

pub async fn get_department_analytics_cached(
    db: &DatabaseConnection,
    query: DepartmentAnalyticsQuery,
) -> Result<DepartmentAnalyticsResponse, DbErr> {
    let mut limit = query.limit.unwrap_or(20);
    if limit == 0 {
        limit = 20;
    }
    let offset = query.offset.unwrap_or(0);
    let live = query.live.unwrap_or(false);

    if let Some(window) = cache_window_for_query(query.start_date, query.end_date) {
        let suffix = department_cache_suffix(&query, limit, offset);
        let key = cache_key(CACHE_CATEGORY_DEPARTMENTS, &window.key, &suffix);
        if !live {
            if let Some(cached) = load_cached_response(db, &key, Some(window.end_date)).await? {
                return Ok(cached);
            }
        }

        let result = analytics_service::get_department_analytics(
            db,
            Some(window.start_date),
            Some(window.end_date),
            Some(limit),
            Some(offset),
            query.search.clone(),
        )
        .await?;
        save_cached_response(db, &key, CACHE_CATEGORY_DEPARTMENTS, &window, &result).await?;
        return Ok(result);
    }

    analytics_service::get_department_analytics(
        db,
        query.start_date,
        query.end_date,
        Some(limit),
        Some(offset),
        query.search,
    )
    .await
}

pub async fn get_timeseries_analytics_cached(
    db: &DatabaseConnection,
    query: TimeSeriesQuery,
) -> Result<TimeSeriesResponse, DbErr> {
    let granularity = query.granularity.clone().unwrap_or_else(|| "day".to_string());
    let live = query.live.unwrap_or(false);

    if let Some(window) = cache_window_for_query(query.start_date, query.end_date) {
        let suffix = timeseries_cache_suffix(&query, &granularity);
        let key = cache_key(CACHE_CATEGORY_TIMESERIES, &window.key, &suffix);
        if !live {
            if let Some(cached) = load_cached_response(db, &key, Some(window.end_date)).await? {
                return Ok(cached);
            }
        }

        let result = analytics_service::get_timeseries_analytics(
            db,
            Some(window.start_date),
            Some(window.end_date),
            granularity,
        )
        .await?;
        save_cached_response(db, &key, CACHE_CATEGORY_TIMESERIES, &window, &result).await?;
        return Ok(result);
    }

    analytics_service::get_timeseries_analytics(db, query.start_date, query.end_date, granularity)
        .await
}

async fn refresh_cache_window(
    db: &DatabaseConnection,
    window: CacheWindow,
) -> Result<(), DbErr> {
    let overview = analytics_service::get_overview_analytics(
        db,
        Some(window.start_date),
        Some(window.end_date),
    )
    .await?;
    let overview_key = cache_key(CACHE_CATEGORY_OVERVIEW, &window.key, "");
    save_cached_response(db, &overview_key, CACHE_CATEGORY_OVERVIEW, &window, &overview).await?;

    let user_query = UserAnalyticsQuery {
        start_date: Some(window.start_date),
        end_date: Some(window.end_date),
        page: Some(0),
        limit: Some(20),
        sort_by: None,
        order: None,
        search: None,
        role: None,
        status: None,
        unassigned_department: None,
        live: None,
    };
    let user_suffix = user_cache_suffix(&user_query, 0, 20);
    let users = analytics_service::calculate_user_analytics(db, user_query, 0, 20).await?;
    let users_key = cache_key(CACHE_CATEGORY_USERS, &window.key, &user_suffix);
    save_cached_response(db, &users_key, CACHE_CATEGORY_USERS, &window, &users).await?;

    let dept_query = DepartmentAnalyticsQuery {
        start_date: Some(window.start_date),
        end_date: Some(window.end_date),
        offset: Some(0),
        limit: Some(20),
        search: None,
        live: None,
        department_id:None,
    };
    let departments = analytics_service::get_department_analytics(
        db,
        Some(window.start_date),
        Some(window.end_date),
        Some(20),
        Some(0),
        None,
    )
    .await?;
    let dept_key = cache_key(
        CACHE_CATEGORY_DEPARTMENTS,
        &window.key,
        &department_cache_suffix(&dept_query, 20, 0),
    );
    save_cached_response(db, &dept_key, CACHE_CATEGORY_DEPARTMENTS, &window, &departments).await?;

    let ts_query = TimeSeriesQuery {
        start_date: Some(window.start_date),
        end_date: Some(window.end_date),
        granularity: Some("day".to_string()),
        group_by: None,
        live: None,
    };
    let timeseries = analytics_service::get_timeseries_analytics(
        db,
        Some(window.start_date),
        Some(window.end_date),
        "day".to_string(),
    )
    .await?;
    let ts_key = cache_key(
        CACHE_CATEGORY_TIMESERIES,
        &window.key,
        &timeseries_cache_suffix(&ts_query, "day"),
    );
    save_cached_response(db, &ts_key, CACHE_CATEGORY_TIMESERIES, &window, &timeseries).await?;

    Ok(())
}

pub async fn refresh_all_analytics_caches(db: &DatabaseConnection) {
    let today = Utc::now().date_naive();

    for days in CACHE_WINDOWS_DAYS {
        let window = cache_window_for_days(days, today);
        if let Err(err) = refresh_cache_window(db, window).await {
            eprintln!("Analytics cache refresh error for {days}d window: {err}");
        }
    }

    let mtd_window = cache_window_for_mtd(today);
    if let Err(err) = refresh_cache_window(db, mtd_window).await {
        eprintln!("Analytics cache refresh error for MTD window: {err}");
    }
}

pub fn spawn_analytics_cache_refresh(db: DatabaseConnection) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(60 * 60));
        loop {
            ticker.tick().await;
            refresh_all_analytics_caches(&db).await;
        }
    });
}
