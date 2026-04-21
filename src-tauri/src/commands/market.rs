use crate::{models::market::MarketServer, services::market_service};

#[tauri::command]
pub async fn list_market_servers(
    q: Option<String>,
    category: Option<String>,
    tag: Option<String>,
) -> Result<Vec<MarketServer>, String> {
    Ok(market_service::list(
        q.as_deref(),
        category.as_deref(),
        tag.as_deref(),
    ))
}

#[tauri::command]
pub async fn get_market_server(name: String) -> Result<Option<MarketServer>, String> {
    Ok(market_service::get(&name))
}

#[tauri::command]
pub async fn get_market_categories() -> Result<Vec<String>, String> {
    Ok(market_service::categories())
}

#[tauri::command]
pub async fn get_market_tags() -> Result<Vec<String>, String> {
    Ok(market_service::tags())
}
