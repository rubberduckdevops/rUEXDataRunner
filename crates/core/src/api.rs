//! UEX `data_submit` client.
//!
//! Builds the JSON payload from an [`Extraction`], submits it with the user's
//! `secret-key`, and parses the report ids from the response. A dry-run mode
//! builds and validates the payload but performs no network call, so the app can
//! be exercised end-to-end without posting to the live community database.

use crate::model::{Extraction, TerminalType};
use crate::trade::TradeEntry;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DEFAULT_BASE_URL: &str = "https://api.uexcorp.space/2.0";

/// Options controlling a submission.
#[derive(Clone, Debug)]
pub struct SubmitOptions {
    pub base_url: String,
    pub secret_key: String,
    /// UEX application API token (Bearer). Required by UEX 2.0 to authorize the app.
    pub api_token: String,
    /// `true` => `is_production=1` (published); `false` => `0` (test row).
    pub is_production: bool,
    /// When set, build the payload but do not perform the HTTP request.
    pub dry_run: bool,
    pub game_version: Option<String>,
}

impl Default for SubmitOptions {
    fn default() -> Self {
        SubmitOptions {
            base_url: DEFAULT_BASE_URL.to_string(),
            secret_key: String::new(),
            api_token: String::new(),
            is_production: true,
            dry_run: true,
            game_version: None,
        }
    }
}

/// Parsed outcome of a submission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmitResponse {
    pub status: String,
    pub http_code: u16,
    pub ids_reports: Vec<String>,
    pub username: Option<String>,
    pub message: String,
    pub date_added: Option<i64>,
    pub dry_run: bool,
}

impl SubmitResponse {
    pub fn is_ok(&self) -> bool {
        self.status == "ok" || self.dry_run
    }
}

/// Build the JSON request body for a submission. `screenshot_b64` is optional
/// (required by UEX for first-time datarunners).
pub fn build_payload(ex: &Extraction, opts: &SubmitOptions, screenshot_b64: Option<&str>) -> Value {
    let ttype = ex.terminal_type.unwrap_or(TerminalType::Sell);
    let mut prices: Vec<Value> = Vec::new();

    for c in &ex.commodities {
        let Some(id) = c.id_commodity else { continue };
        if !c.include {
            continue;
        }
        // Skip rows with no usable data at all.
        if c.status.is_none() && c.price.is_none() && c.quantity_scu.is_none() {
            continue;
        }
        let price = c.price.unwrap_or(0);
        let scu = c.quantity_scu.unwrap_or(0);
        let st = c.status.unwrap_or(1);
        let row = match ttype {
            TerminalType::Buy => json!({
                "id_commodity": id,
                "price_buy": price,
                "scu_buy": scu,
                "status_buy": st,
                "is_missing": 0,
            }),
            TerminalType::Sell => json!({
                "id_commodity": id,
                "price_sell": price,
                "scu_sell": scu,
                "status_sell": st,
                "is_missing": 0,
            }),
        };
        prices.push(row);
    }

    let mut body = json!({
        "id_terminal": ex.id_terminal,
        "type": "commodity",
        "is_production": if opts.is_production { 1 } else { 0 },
        "prices": prices,
    });

    if let Some(v) = &opts.game_version {
        body["game_version"] = json!(v);
    }
    if let Some(ts) = ex.captured_at {
        body["date_added"] = json!(ts);
    }
    if let Some(s) = screenshot_b64 {
        body["screenshot"] = json!(s);
    }
    body
}

/// The number of commodity rows that will actually be submitted.
pub fn submittable_row_count(ex: &Extraction) -> usize {
    ex.commodities
        .iter()
        .filter(|c| {
            c.id_commodity.is_some()
                && c.include
                && !(c.status.is_none() && c.price.is_none() && c.quantity_scu.is_none())
        })
        .count()
}

/// Submit an extraction to UEX (or simulate it in dry-run mode).
pub fn submit(
    ex: &Extraction,
    opts: &SubmitOptions,
    screenshot_b64: Option<&str>,
) -> anyhow::Result<SubmitResponse> {
    if ex.id_terminal.is_none() {
        anyhow::bail!("cannot submit: terminal not identified");
    }
    let payload = build_payload(ex, opts, screenshot_b64);

    if opts.dry_run {
        return Ok(SubmitResponse {
            status: "dry_run".to_string(),
            http_code: 0,
            ids_reports: Vec::new(),
            username: None,
            message: format!(
                "Dry run: {} row(s) prepared, not submitted",
                submittable_row_count(ex)
            ),
            date_added: ex.captured_at,
            dry_run: true,
        });
    }

    if opts.secret_key.trim().is_empty() {
        anyhow::bail!("cannot submit: UEX secret key not configured (Settings)");
    }
    if opts.api_token.trim().is_empty() {
        anyhow::bail!(
            "cannot submit: UEX application API token not set — create an app at \
             https://uexcorp.space/api/apps and paste its token in Settings"
        );
    }

    let url = format!("{}/data_submit", opts.base_url.trim_end_matches('/'));
    let mut req = ureq::post(&url)
        .set("secret-key", opts.secret_key.trim())
        .set("Content-Type", "application/json");
    // UEX 2.0 requires an application Bearer token to authorize the request.
    // Tolerate a pasted "Bearer " prefix and stray whitespace.
    let token = opts
        .api_token
        .trim()
        .trim_start_matches("Bearer ")
        .trim_start_matches("bearer ")
        .trim();
    if !token.is_empty() {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let result = req.send_json(payload);

    let (http_code, body_text) = match result {
        Ok(resp) => (resp.status(), resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, resp)) => (code, resp.into_string().unwrap_or_default()),
        Err(e) => return Err(anyhow::anyhow!("network error: {e}")),
    };

    parse_response(http_code, &body_text)
}

/// Parse a UEX `data_submit` response body.
pub fn parse_response(http_code: u16, body: &str) -> anyhow::Result<SubmitResponse> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("invalid JSON response ({http_code}): {e}; body={body}"))?;

    let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("error").to_string();
    let message = v.get("message").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let data = v.get("data");

    let ids_reports = data
        .and_then(|d| d.get("ids_reports"))
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(value_to_string).collect())
        .unwrap_or_default();

    let username = data
        .and_then(|d| d.get("username"))
        .and_then(value_to_string);

    let date_added = data
        .and_then(|d| d.get("date_added"))
        .and_then(value_to_i64);

    Ok(SubmitResponse {
        status,
        http_code,
        ids_reports,
        username,
        message,
        date_added,
        dry_run: false,
    })
}

fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Personal trade log (UEX user_trades_add)
// ---------------------------------------------------------------------------

/// Parsed outcome of adding a trade entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeResponse {
    pub status: String,
    pub http_code: u16,
    /// UEX `id_user_trade` returned on success.
    pub id_user_trade: Option<i64>,
    pub message: String,
    pub dry_run: bool,
}

impl TradeResponse {
    pub fn is_ok(&self) -> bool {
        self.status == "ok" || self.dry_run
    }
}

/// Build the `user_trades_add` request body.
pub fn build_trade_payload(entry: &TradeEntry, is_production: bool) -> Value {
    json!({
        "is_production": if is_production { 1 } else { 0 },
        "id_terminal": entry.id_terminal,
        "id_commodity": entry.id_commodity,
        "operation": entry.operation.as_str(),
        "scu": entry.scu,
        "price": entry.price,
    })
}

/// Add a trade entry to the user's UEX journal (or simulate it in dry-run mode).
pub fn submit_trade(entry: &TradeEntry, opts: &SubmitOptions) -> anyhow::Result<TradeResponse> {
    if entry.id_terminal == 0 || entry.id_commodity == 0 {
        anyhow::bail!("cannot log trade: commodity and terminal must be selected");
    }
    let payload = build_trade_payload(entry, opts.is_production);

    if opts.dry_run {
        return Ok(TradeResponse {
            status: "dry_run".to_string(),
            http_code: 0,
            id_user_trade: None,
            message: "Dry run: trade prepared, not sent".to_string(),
            dry_run: true,
        });
    }
    if opts.secret_key.trim().is_empty() {
        anyhow::bail!("cannot log trade: UEX secret key not configured (Settings)");
    }
    if opts.api_token.trim().is_empty() {
        anyhow::bail!("cannot log trade: UEX application API token not set (Settings)");
    }

    let url = format!("{}/user_trades_add", opts.base_url.trim_end_matches('/'));
    let token = opts.api_token.trim().trim_start_matches("Bearer ").trim();
    let result = ureq::post(&url)
        .set("secret-key", opts.secret_key.trim())
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_json(payload);

    let (http_code, body_text) = match result {
        Ok(resp) => (resp.status(), resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, resp)) => (code, resp.into_string().unwrap_or_default()),
        Err(e) => return Err(anyhow::anyhow!("network error: {e}")),
    };
    parse_trade_response(http_code, &body_text)
}

/// Parse a `user_trades_add` response. `data` may be the numeric id directly or
/// an object containing `id_user_trade`.
pub fn parse_trade_response(http_code: u16, body: &str) -> anyhow::Result<TradeResponse> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("invalid JSON response ({http_code}): {e}; body={body}"))?;
    let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("error").to_string();
    let message = v.get("message").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let data = v.get("data");
    let id_user_trade = data.and_then(|d| match d {
        Value::Number(_) | Value::String(_) => value_to_i64(d),
        Value::Object(_) => d.get("id_user_trade").and_then(value_to_i64),
        _ => None,
    });
    Ok(TradeResponse { status, http_code, id_user_trade, message, dry_run: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Commodity;

    fn buy_extraction() -> Extraction {
        let mut ex = Extraction::new("t.jpg");
        ex.id_terminal = Some(551);
        ex.terminal_type = Some(TerminalType::Buy);
        ex.captured_at = Some(1_786_594_856);
        let mut c = Commodity::new("Diamond");
        c.id_commodity = Some(25);
        c.price = Some(6561);
        c.quantity_scu = Some(525);
        c.status = Some(7);
        ex.commodities.push(c);
        ex
    }

    #[test]
    fn payload_uses_buy_fields() {
        let ex = buy_extraction();
        let opts = SubmitOptions::default();
        let p = build_payload(&ex, &opts, Some("BASE64"));
        assert_eq!(p["id_terminal"], json!(551));
        assert_eq!(p["type"], json!("commodity"));
        assert_eq!(p["date_added"], json!(1_786_594_856_i64));
        assert_eq!(p["screenshot"], json!("BASE64"));
        let row = &p["prices"][0];
        assert_eq!(row["id_commodity"], json!(25));
        assert_eq!(row["price_buy"], json!(6561));
        assert_eq!(row["scu_buy"], json!(525));
        assert_eq!(row["status_buy"], json!(7));
        assert!(row.get("price_sell").is_none());
    }

    #[test]
    fn payload_uses_sell_fields() {
        let mut ex = buy_extraction();
        ex.terminal_type = Some(TerminalType::Sell);
        let p = build_payload(&ex, &SubmitOptions::default(), None);
        let row = &p["prices"][0];
        assert_eq!(row["price_sell"], json!(6561));
        assert!(row.get("price_buy").is_none());
        assert!(p.get("screenshot").is_none());
    }

    #[test]
    fn excluded_rows_are_dropped() {
        let mut ex = buy_extraction();
        ex.commodities[0].include = false;
        let p = build_payload(&ex, &SubmitOptions::default(), None);
        assert_eq!(p["prices"].as_array().unwrap().len(), 0);
        assert_eq!(submittable_row_count(&ex), 0);
    }

    #[test]
    fn dry_run_does_not_require_key_and_reports_count() {
        let ex = buy_extraction();
        let opts = SubmitOptions { dry_run: true, ..Default::default() };
        let r = submit(&ex, &opts, None).unwrap();
        assert!(r.dry_run);
        assert!(r.is_ok());
        assert!(r.message.contains("1 row"));
    }

    #[test]
    fn parses_real_response() {
        let body = r#"{"status":"ok","http_code":200,"data":{"ids_reports":["958660","958662"],"date_added":"1786594856","username":"patches_124454"},"message":""}"#;
        let r = parse_response(200, body).unwrap();
        assert_eq!(r.status, "ok");
        assert_eq!(r.ids_reports, vec!["958660", "958662"]);
        assert_eq!(r.username.as_deref(), Some("patches_124454"));
        assert_eq!(r.date_added, Some(1_786_594_856));
        assert!(r.is_ok());
    }

    #[test]
    fn trade_payload_and_dry_run() {
        let entry = crate::trade::new_entry(
            crate::trade::TradeOp::Buy, 18, "Compboard", 29, "ARC-L3", "Stanton", 110, 2441.0, 1,
        );
        let p = build_trade_payload(&entry, true);
        assert_eq!(p["id_terminal"], json!(29));
        assert_eq!(p["id_commodity"], json!(18));
        assert_eq!(p["operation"], json!("buy"));
        assert_eq!(p["scu"], json!(110));
        assert_eq!(p["price"], json!(2441.0));
        assert_eq!(p["is_production"], json!(1));

        let opts = SubmitOptions { dry_run: true, ..Default::default() };
        let r = submit_trade(&entry, &opts).unwrap();
        assert!(r.dry_run && r.is_ok());
    }

    #[test]
    fn parses_trade_response_variants() {
        let a = parse_trade_response(200, r#"{"status":"ok","data":{"id_user_trade":8123},"message":""}"#).unwrap();
        assert_eq!(a.id_user_trade, Some(8123));
        assert!(a.is_ok());
        let b = parse_trade_response(200, r#"{"status":"ok","data":8124,"message":""}"#).unwrap();
        assert_eq!(b.id_user_trade, Some(8124));
    }

    #[test]
    fn parses_error_response() {
        let body = r#"{"status":"missing_secret_key","http_code":400,"data":null,"message":"secret key required"}"#;
        let r = parse_response(400, body).unwrap();
        assert_eq!(r.status, "missing_secret_key");
        assert!(!r.is_ok());
        assert!(r.ids_reports.is_empty());
    }
}
