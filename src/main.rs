use std::{io::Stdout, time::Duration};
use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::{task, time};
use reqwest::Client;
use ratatui::{prelude::*, backend::CrosstermBackend, widgets::{Block, Borders, Row, Table}};
use crossterm::{execute, event, terminal};

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
enum AssetType {
    Stock,
    Crypto,
    Commodity,
}

#[derive(Debug, Deserialize, Clone)]
struct AssetConfig {
    kind: AssetType,
    symbol: String,
    quantity: f64,
    api: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Portfolio {
    assets: Vec<AssetConfig>,
}

// Data retrieval
async fn load_config(path: &str) -> Result<Vec<AssetConfig>> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("reading {path}"))?;
    let portfolio: Portfolio = toml::from_str(&raw).context("parsing TOML portfolio file")?;
    Ok(portfolio.assets)
}

async fn fetch_price(client: &Client, a: &AssetConfig) -> Result<f64> {
    const API_KEY: &str = "0ac51e33-41f2-414a-90c4-3301efbbce7c";
    let url = if let Some(custom) = &a.api {
        custom.clone()
    } else {
        match a.kind {
            AssetType::Stock => format!("https://example.com/stock/{}", a.symbol),
            AssetType::Crypto => format!(
                "https://pro-api.coinmarketcap.com/v1/cryptocurrency/quotes/latest?symbol={symbol}&CMC_PRO_API_KEY={key}",
                symbol = a.symbol,
                key = API_KEY
            ),
            AssetType::Commodity => format!("https://example.com/commodity/{}", a.symbol),
        }
    };

    #[derive(Deserialize)]
    struct CmcRespInner {
        #[serde(rename = "quote")]
        quote: std::collections::HashMap<String, serde_json::Value>,
    }

    #[derive(Deserialize)]
    struct CmcResponse {
        data: std::collections::HashMap<String, CmcRespInner>,
    }

    if matches!(a.kind, AssetType::Crypto) {
        let resp: CmcResponse = client.get(url).send().await?.json().await?;
        let inner = resp
            .data
            .get(&a.symbol)
            .context("symbol missing")?;
        let usd = inner.quote.get("USD").context("USD quote missing")?;
        let price = usd.get("price").context("price missing")?.as_f64().context("not f64")?;
        return Ok(price);
    }

    #[derive(Deserialize)]
    struct Resp {
        price: f64,
    }

    let price = client.get(url).send().await?.json::<Resp>().await?.price;
    Ok(price)
}

async fn refresh_portfolio(client: &Client, cfg: &[AssetConfig]) -> Vec<(AssetConfig, f64)> {
    let tasks = cfg.iter().cloned().map(|asset| {
        let c = client.clone();
        task::spawn(async move {
            let price = fetch_price(&c, &asset).await.unwrap_or(0.0);
            (asset, price)
        })
    });

    futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|r| r.expect("task panicked"))
        .collect()
}

// Rendering

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> Result<Term> {
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen, event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(mut term: Term) -> Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        terminal::LeaveAlternateScreen,
        event::DisableMouseCapture
    )?;
    term.show_cursor()?;
    Ok(())
}

fn draw_ui(f: &mut Frame, rows: &[(AssetConfig, f64)]) {
    let header = Row::new(["Type", "Symbol", "Qty", "Price", "Value"]).style(
        Style::default().add_modifier(Modifier::BOLD),
    );

    let body = rows.iter().map(|(a, price)| {
        Row::new([
            format!("{:?}", a.kind),
            a.symbol.clone(),
            a.quantity.to_string(),
            format!("{:.2}", price),
            format!("{:.2}", price * a.quantity),
        ])
    });

    let widths = [
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(14),
    ];

    let table = Table::new(body, widths)
        .header(header)
        .block(Block::default().title("Portfolio").borders(Borders::ALL));

    f.render_widget(table, f.size());
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = load_config("portfolio.toml").await?;
    let client = Client::builder().user_agent("sodatte/0.1").build()?;

    let mut term = setup_terminal()?;

    let mut ticker = time::interval(Duration::from_secs(30));

    loop {
        ticker.tick().await;

        let rows = refresh_portfolio(&client, &cfg).await;

        term.draw(|f| draw_ui(f, &rows))?;

        if event::poll(Duration::from_millis(100))? {
            if let event::Event::Key(key) = event::read()? {
                if key.code == event::KeyCode::Char('q') || key.code == event::KeyCode::Esc {
                    break;
                }
            }
        }
    }

    restore_terminal(term)?;
    Ok(())
}
