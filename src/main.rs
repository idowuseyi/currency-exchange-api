// src/main.rs
use actix_web::{web, App, HttpResponse, HttpServer, Responder, middleware::Logger};
use actix_web::http::StatusCode;
use chrono::{DateTime, Utc};
use rand::Rng;
use reqwest;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, MySqlPool, Pool, Row};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use anyhow::Result as AnyResult;
use plotters::prelude::*;
use actix_web::dev::Server;
use serde_json::json;

#[derive(Deserialize)]
struct ApiCountry {
    name: String,
    capital: Option<String>,
    region: Option<String>,
    population: u64,
    flag: String,
    currencies: Vec<Currency>,
}

#[derive(Deserialize)]
struct Currency {
    #[serde(rename = "code")]
    code: Option<String>,
}

#[derive(Deserialize)]
struct ExchangeRates {
    base: String,
    date: String,
    rates: HashMap<String, f64>,
}

#[derive(sqlx::FromRow, Serialize, Clone)]
struct Country {
    id: Option<i32>,
    name: String,
    capital: Option<String>,
    region: Option<String>,
    population: i64,
    currency_code: Option<String>,
    exchange_rate: Option<f64>,
    estimated_gdp: f64,
    flag_url: Option<String>,
    last_refreshed_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct QueryParams {
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    sort: Option<String>,
}

#[derive(Serialize)]
struct StatusResponse {
    total_countries: i64,
    last_refreshed_at: Option<String>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

async fn refresh_handler(
    pool: web::Data<Pool<MySql>>,
) -> Result<impl Responder, actix_web::Error> {
    let countries_url = "https://restcountries.com/v2/all?fields=name,capital,region,population,flag,currencies";
    let rates_url = "https://open.er-api.com/v6/latest/USD";

    // Fetch countries
    let api_countries: AnyResult<Vec<ApiCountry>> = async {
        let resp = reqwest::get(countries_url).await.map_err(|e| anyhow::anyhow!("Failed to fetch countries: {}", e))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch from restcountries.com"));
        }
        resp.json().await.map_err(|e| anyhow::anyhow!("Failed to parse countries: {}", e))
    }.await;

    let api_countries = match api_countries {
        Ok(countries) => countries,
        Err(e) => {
            return Ok(HttpResponse::ServiceUnavailable().json(ErrorResponse {
                error: "External data source unavailable".to_string(),
                details: Some(format!("Could not fetch data from restcountries.com: {}", e)),
            }));
        }
    };

    // Fetch exchange rates
    let rates_resp: AnyResult<ExchangeRates> = async {
        let resp = reqwest::get(rates_url).await.map_err(|e| anyhow::anyhow!("Failed to fetch rates: {}", e))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch from open.er-api.com"));
        }
        resp.json().await.map_err(|e| anyhow::anyhow!("Failed to parse rates: {}", e))
    }.await;

    let rates = match rates_resp {
        Ok(r) => r,
        Err(e) => {
            return Ok(HttpResponse::ServiceUnavailable().json(ErrorResponse {
                error: "External data source unavailable".to_string(),
                details: Some(format!("Could not fetch data from open.er-api.com: {}", e)),
            }));
        }
    };

    let now = Utc::now();
    let mut processed_countries: Vec<Country> = vec![];

    for api_c in api_countries {
        // Validation: skip if required fields missing
        if api_c.name.trim().is_empty() || api_c.population == 0 {
            continue;
        }

        let currency_code: Option<String> = if api_c.currencies.is_empty() {
            None
        } else {
            api_c.currencies[0].code.clone().filter(|c| !c.trim().is_empty())
        };

        let (exchange_rate, estimated_gdp) = if let Some(ref code) = currency_code {
            rates.rates.get(code.as_str()).map(|rate| {
                let multiplier: f64 = rand::thread_rng().gen_range(1000.0..=2000.0);
                let gdp = api_c.population as f64 * multiplier / *rate;
                (*rate, gdp)
            }).unwrap_or((0.0, 0.0))
        } else {
            (0.0, 0.0)
        };

        let exchange_rate_opt = if exchange_rate > 0.0 { Some(exchange_rate) } else { None };
        let estimated_gdp_val = if exchange_rate > 0.0 { estimated_gdp } else { 0.0 };

        let country = Country {
            id: None,
            name: api_c.name,
            capital: api_c.capital,
            region: api_c.region,
            population: api_c.population as i64,
            currency_code,
            exchange_rate: exchange_rate_opt,
            estimated_gdp: estimated_gdp_val,
            flag_url: Some(api_c.flag),
            last_refreshed_at: now,
        };

        processed_countries.push(country);
    }

    // Store in DB using transaction
    match store_countries(&pool, &processed_countries, &now).await {
        Ok(_) => {
            // Generate image
            if let Err(e) = generate_summary_image(&pool, &now).await {
                log::warn!("Failed to generate summary image: {}", e);
            }
            Ok(HttpResponse::Ok().json(json!({"status": "success", "refreshed_at": now.to_rfc3339()})))
        }
        Err(e) => {
            Ok(HttpResponse::InternalServerError().json(ErrorResponse {
                error: "Internal server error".to_string(),
                details: None,
            }))
        }
    }
}

async fn store_countries(
    pool: &Pool<MySql>,
    countries: &[Country],
    now: &DateTime<Utc>,
) -> AnyResult<()> {
    let mut tx = pool.begin().await?;

    for country in countries {
        // Check if exists (case-insensitive)
        let exists_row = sqlx::query("SELECT id FROM countries WHERE LOWER(name) = LOWER($1)")
            .bind(&country.name)
            .fetch_optional(&mut *tx)
            .await?;

        if let Some(row) = exists_row {
            let id: i32 = row.get(0);
            // Update
            sqlx::query(
                "UPDATE countries SET capital = $1, region = $2, population = $3, currency_code = $4, exchange_rate = $5, estimated_gdp = $6, flag_url = $7, last_refreshed_at = $8 WHERE id = $9"
            )
            .bind(&country.capital)
            .bind(&country.region)
            .bind(country.population)
            .bind(&country.currency_code)
            .bind(&country.exchange_rate)
            .bind(country.estimated_gdp)
            .bind(&country.flag_url)
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        } else {
            // Insert
            sqlx::query(
                "INSERT INTO countries (name, capital, region, population, currency_code, exchange_rate, estimated_gdp, flag_url, last_refreshed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
            )
            .bind(&country.name)
            .bind(&country.capital)
            .bind(&country.region)
            .bind(country.population)
            .bind(&country.currency_code)
            .bind(&country.exchange_rate)
            .bind(country.estimated_gdp)
            .bind(&country.flag_url)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

async fn generate_summary_image(pool: &Pool<MySql>, now: &DateTime<Utc>) -> AnyResult<()> {
    fs::create_dir_all("cache").map_err(|e| anyhow::anyhow!("Failed to create cache dir: {}", e))?;

    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM countries")
        .fetch_one(pool)
        .await?;

    let top5_rows = sqlx::query("SELECT name, estimated_gdp FROM countries ORDER BY estimated_gdp DESC LIMIT 5")
        .fetch_all(pool)
        .await?;

    let mut top5 = vec![];
    for row in top5_rows {
        let name: String = row.get(0);
        let gdp: f64 = row.get(1);
        top5.push((name, gdp));
    }

    let timestamp = now.to_rfc3339();

    let root_area = BitMapBackend::new("cache/summary.png", (800, 600))
        .into_drawing_area();
    root_area.fill(&WHITE)?;

    let font_style_title = TextStyle::from(("sans-serif", 30).into_font())
        .color(&BLACK);
    root_area.draw_text(
        "Country Summary",
        &font_style_title,
        Coord::new(50f64, 50f64, BackendCoord::default()),
    )?;

    let font_style_normal = TextStyle::from(("sans-serif", 20).into_font())
        .color(&BLACK);
    root_area.draw_text(
        &format!("Total Countries: {}", total),
        &font_style_normal,
        Coord::new(50f64, 100f64, BackendCoord::default()),
    )?;

    let mut y_pos = 150f64;
    let font_style_list = TextStyle::from(("sans-serif", 16).into_font())
        .color(&BLACK);
    for (name, gdp) in top5 {
        root_area.draw_text(
            &format!("{}: {:.2}", name, gdp),
            &font_style_list,
            Coord::new(50f64, y_pos, BackendCoord::default()),
        )?;
        y_pos += 30.0;
    }

    root_area.draw_text(
        &format!("Last Refreshed: {}", timestamp),
        &font_style_list,
        Coord::new(50f64, y_pos + 50.0, BackendCoord::default()),
    )?;

    root_area.present().map_err(|e| anyhow::anyhow!("Failed to present image: {}", e))?;
    Ok(())
}

async fn get_countries(
    pool: web::Data<Pool<MySql>>,
    web::Query(params): web::Query<QueryParams>,
) -> impl Responder {
    let mut sql = String::from("SELECT id, name, capital, region, population, currency_code, exchange_rate, estimated_gdp, flag_url, last_refreshed_at FROM countries");

    let mut where_clauses = vec![];
    let mut binds: Vec<&(dyn sqlx::Encode<'_, sqlx::MySql> + sqlx::Type<sqlx::MySql> + Sync)> = vec![];

    if let Some(ref region) = params.region {
        if !region.trim().is_empty() {
            where_clauses.push("region = ?");
            binds.push(region);
        }
    }

    if let Some(ref currency) = params.currency {
        if !currency.trim().is_empty() {
            where_clauses.push("currency_code = ?");
            binds.push(currency);
        }
    }

    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }

    match params.sort.as_deref() {
        Some("gdp_desc") => sql.push_str(" ORDER BY estimated_gdp DESC"),
        Some("gdp_asc") => sql.push_str(" ORDER BY estimated_gdp ASC"),
        _ => {}
    }

    let mut query = sqlx::query_as::<_, Country>(&sql);

    for bind in binds {
        query = query.bind(bind);
    }

    match query.fetch_all(&**pool).await {
        Ok(countries) => HttpResponse::Ok().json(countries),
        Err(_) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: "Internal server error".to_string(),
            details: None,
        }),
    }
}

async fn get_country(
    pool: web::Data<Pool<MySql>>,
    path: web::Path<String>,
) -> impl Responder {
    let name = path.into_inner();
    if name.trim().is_empty() {
        return HttpResponse::BadRequest().json(ErrorResponse {
            error: "Validation failed".to_string(),
            details: Some("name is required".to_string()),
        });
    }

    let row = sqlx::query_as::<_, Country>(
        "SELECT id, name, capital, region, population, currency_code, exchange_rate, estimated_gdp, flag_url, last_refreshed_at FROM countries WHERE LOWER(name) = LOWER($1)"
    )
    .bind(&name)
    .fetch_optional(&**pool)
    .await;

    match row {
        Ok(Some(country)) => HttpResponse::Ok().json(country),
        Ok(None) => HttpResponse::NotFound().json(ErrorResponse {
            error: "Country not found".to_string(),
            details: None,
        }),
        Err(_) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: "Internal server error".to_string(),
            details: None,
        }),
    }
}

async fn delete_country(
    pool: web::Data<Pool<MySql>>,
    path: web::Path<String>,
) -> impl Responder {
    let name = path.into_inner();
    if name.trim().is_empty() {
        return HttpResponse::BadRequest().json(ErrorResponse {
            error: "Validation failed".to_string(),
            details: Some("name is required".to_string()),
        });
    }

    let result = sqlx::query("DELETE FROM countries WHERE LOWER(name) = LOWER($1)")
        .bind(&name)
        .execute(&**pool)
        .await;

    match result {
        Ok(res) if res.rows_affected() > 0 => HttpResponse::Ok().finish(),
        Ok(_) => HttpResponse::NotFound().json(ErrorResponse {
            error: "Country not found".to_string(),
            details: None,
        }),
        Err(_) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: "Internal server error".to_string(),
            details: None,
        }),
    }
}

async fn status_handler(pool: web::Data<Pool<MySql>>) -> impl Responder {
    let total_result: Result<(i64,), sqlx::Error> = sqlx::query_as("SELECT COUNT(*) FROM countries")
        .fetch_one(&**pool)
        .await;

    let last_result: Result<Option<(DateTime<Utc>,)>, sqlx::Error> = sqlx::query_as("SELECT MAX(last_refreshed_at) FROM countries")
        .fetch_optional(&**pool)
        .await;

    match (total_result, last_result) {
        (Ok((total,)), Ok(last_opt)) => {
            let last_str = last_opt.map(|(dt,)| dt.to_rfc3339());
            HttpResponse::Ok().json(StatusResponse {
                total_countries: total,
                last_refreshed_at: last_str,
            })
        }
        _ => HttpResponse::InternalServerError().json(ErrorResponse {
            error: "Internal server error".to_string(),
            details: None,
        }),
    }
}

async fn image_handler() -> impl Responder {
    match fs::read("cache/summary.png") {
        Ok(data) => HttpResponse::Ok()
            .content_type("image/png")
            .body(data),
        Err(_) => HttpResponse::NotFound().json(ErrorResponse {
            error: "Summary image not found".to_string(),
            details: None,
        }),
    }
}

#[actix_web::main]
async fn main() -> io::Result<()> {
    env::set_var("RUST_LOG", "info");
    env_logger::init();

    dotenv::dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = MySqlPool::connect(&database_url).await.expect("Failed to connect to DB");

    let port_str = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let port: u16 = port_str.parse().expect("PORT must be a valid number");

    println!("Starting server on port {}", port);

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .wrap(Logger::default())
            .service(
                web::scope("/countries")
                    .route("/refresh", web::post().to(refresh_handler))
                    .route("", web::get().to(get_countries))
                    .route("/{name}", web::get().to(get_country))
                    .route("/{name}", web::delete().to(delete_country))
                    .route("/image", web::get().to(image_handler))
            )
            .route("/status", web::get().to(status_handler))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}