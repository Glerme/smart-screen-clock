#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::{imageops, AnimationDecoder, Frame};
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::Manager;

/// Corta o centro do GIF em quadrado e redimensiona para `size` x `size`, mantendo a animação.
fn crop_gif_square(bytes: &[u8], size: u32) -> Result<Vec<u8>, String> {
    let frames = GifDecoder::new(Cursor::new(bytes))
        .and_then(|d| d.into_frames().collect_frames())
        .map_err(|e| format!("GIF inválido: {e}"))?;
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 10);
        enc.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;
        for f in frames {
            let delay = f.delay();
            let mut buf = f.into_buffer();
            let (w, h) = buf.dimensions();
            let s = w.min(h);
            let sq = imageops::crop(&mut buf, (w - s) / 2, (h - s) / 2, s, s).to_image();
            let small = imageops::resize(&sq, size, size, imageops::FilterType::Triangle);
            enc.encode_frame(Frame::from_parts(small, 0, 0, delay))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

// async: comandos síncronos rodam na thread principal e congelariam a janela durante o recorte
#[tauri::command]
async fn crop_gif(request: Request<'_>) -> Result<Response, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("esperava os bytes do GIF".into());
    };
    // Tamanho vem no cabeçalho "size": 80 para a animação do relógio, 240 para a tela Photo
    let size = match request.headers().get("size").and_then(|v| v.to_str().ok()) {
        Some("240") => 240,
        _ => 80,
    };
    crop_gif_square(bytes, size).map(Response::new)
}

/// Uso do plano Claude (janelas de 5h e semanal), via o mesmo endpoint do /usage do Claude Code.
/// shortcut: endpoint não documentado; se mudar, o painel mostra o erro em vez dos números.
async fn claude_usage(home: &Path) -> Result<Value, String> {
    let creds = std::fs::read_to_string(home.join(".claude/.credentials.json"))
        .map_err(|_| "sem login do Claude Code")?;
    let creds: Value = serde_json::from_str(&creds).map_err(|e| e.to_string())?;
    let token = creds["claudeAiOauth"]["accessToken"]
        .as_str()
        .ok_or("sem token do Claude")?;
    let res = tauri_plugin_http::reqwest::Client::new()
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("sem conexão: {e}"))?;
    // Não renovamos o token aqui para não mexer no arquivo de login do Claude Code
    if res.status() == 401 {
        return Err("token expirado: abra o Claude Code".into());
    }
    if !res.status().is_success() {
        return Err(format!("Anthropic respondeu {}", res.status()));
    }
    let u: Value = serde_json::from_str(&res.text().await.map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let win = |k: &str, label: &str| {
        json!({ "label": label, "pct": u[k]["utilization"], "resets_at": u[k]["resets_at"] })
    };
    Ok(json!([win("five_hour", "5h"), win("seven_day", "Semana")]))
}

fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m
            .get(key)
            .filter(|x| x.is_object())
            .or_else(|| m.values().find_map(|x| find_key(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_key(x, key)),
        _ => None,
    }
}

fn collect_jsonl(dir: &Path, out: &mut Vec<(SystemTime, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().is_some_and(|x| x == "jsonl") {
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((modified, path));
        }
    }
}

/// Limites do Codex lidos dos registros de sessão locais (sem token). Só atualizam quando o Codex é usado.
fn codex_usage(home: &Path) -> Result<Value, String> {
    let mut files = Vec::new();
    collect_jsonl(&home.join(".codex/sessions"), &mut files);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, f) in files.iter().take(20) {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        let found = text
            .lines()
            .rev()
            .filter(|l| l.contains("\"rate_limits\""))
            .find_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                find_key(&v, "rate_limits").cloned()
            });
        if let Some(rl) = found {
            let win = |k: &str| {
                let minutes = rl[k]["window_minutes"].as_i64().unwrap_or(0);
                let label = if minutes >= 10080 { "Semana".to_string() } else { format!("{}h", minutes / 60) };
                json!({
                    "label": label,
                    "pct": rl[k]["used_percent"],
                    "resets_at": rl[k]["resets_at"].as_i64().map(|s| s * 1000),
                })
            };
            return Ok(json!([win("primary"), win("secondary")]));
        }
    }
    Err("sem dados: use o Codex uma vez".into())
}

#[tauri::command]
async fn ai_usage(app: tauri::AppHandle) -> Value {
    let home = app.path().home_dir().unwrap_or_default();
    let pack = |r: Result<Value, String>| match r {
        Ok(w) => json!({ "windows": w }),
        Err(e) => json!({ "error": e }),
    };
    json!({ "claude": pack(claude_usage(&home).await), "codex": pack(codex_usage(&home)) })
}

/// CPU, RAM, disco (/) e temperatura da CPU neste PC.
#[tauri::command]
async fn pc_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut sys = sysinfo::System::new();
        // A CPU precisa de duas leituras com intervalo para calcular o uso
        sys.refresh_cpu_usage();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(500)));
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let gb = |b: u64| b as f64 / 1e9;
        let (total, avail) = (sys.total_memory(), sys.available_memory());
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let root = disks.list().iter().find(|d| d.mount_point() == Path::new("/"));
        let temps = sysinfo::Components::new_with_refreshed_list();
        let cpu_temp = |want: &[&str]| {
            temps
                .list()
                .iter()
                .filter(|c| want.is_empty() || want.iter().any(|w| c.label().contains(w)))
                .filter_map(|c| c.temperature())
                .fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.max(t))))
        };
        // Tctl (AMD) / Package (Intel) é a temperatura da CPU; senão, a maior disponível
        let temp = cpu_temp(&["Tctl", "Package", "k10temp", "coretemp"]).or_else(|| cpu_temp(&[]));
        json!({
            "cpu": sys.global_cpu_usage(),
            "ram": if total > 0 { (total - avail) as f64 / total as f64 * 100.0 } else { 0.0 },
            "ram_used_gb": gb(total - avail),
            "ram_total_gb": gb(total),
            "disk": root.map(|d| (d.total_space() - d.available_space()) as f64 / d.total_space().max(1) as f64 * 100.0),
            "disk_free_gb": root.map(|d| gb(d.available_space())),
            "temp": temp,
        })
    })
    .await
    .map_err(|e| e.to_string())
}

const GITHUB_QUERY: &str = r#"query {
  viewer {
    login
    pullRequests(states: OPEN, first: 3, orderBy: {field: UPDATED_AT, direction: DESC}) {
      totalCount
      nodes { title number repository { name } commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } }
    }
  }
  search(query: "is:open is:pr review-requested:@me archived:false", type: ISSUE, first: 1) { issueCount }
}"#;

/// PRs abertos do usuário, revisões pedidas a ele e o CI de cada PR, via o `gh` já logado.
#[tauri::command]
async fn github_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let out = std::process::Command::new("gh")
            .args(["api", "graphql", "-f", &format!("query={GITHUB_QUERY}")])
            .output()
            .map_err(|_| "gh não encontrado".to_string())?;
        if !out.status.success() {
            return Err("gh sem login ou sem rede".to_string());
        }
        let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        let prs = &v["data"]["viewer"]["pullRequests"];
        let list: Vec<Value> = prs["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                json!({
                    "title": p["title"],
                    "repo": p["repository"]["name"],
                    "number": p["number"],
                    "ci": p["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"],
                })
            })
            .collect();
        Ok(json!({
            "login": v["data"]["viewer"]["login"],
            "open": prs["totalCount"],
            "reviews": v["data"]["search"]["issueCount"],
            "prs": list,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    tauri::Builder::default()
        // HTTP pelo Rust: a tela não envia cabeçalhos CORS, então fetch do webview falharia
        .plugin(tauri_plugin_http::init())
        .invoke_handler(tauri::generate_handler![crop_gif, ai_usage, pc_status, github_status])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o app");
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, RgbaImage};

    #[test]
    fn corta_gif_retangular_para_80x80_mantendo_frames() {
        let mut src = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut src);
            for c in [0u8, 255] {
                let img = RgbaImage::from_pixel(160, 100, image::Rgba([c, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1))).unwrap();
            }
        }
        let out = crop_gif_square(&src, 80).unwrap();
        let frames = GifDecoder::new(Cursor::new(out)).unwrap().into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.buffer().dimensions() == (80, 80)));
    }

    #[test]
    fn le_limites_do_codex_da_sessao_mais_recente() {
        let home = std::env::temp_dir().join(format!("smartscreen-test-{}", std::process::id()));
        let dir = home.join(".codex/sessions/2026/10/22");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rollout.jsonl"),
            concat!(
                "{\"type\":\"other\"}\n",
                "{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":1.0,\"window_minutes\":300,\"resets_at\":100},",
                "\"secondary\":{\"used_percent\":40.0,\"window_minutes\":10080,\"resets_at\":200}}}}\n",
                "{\"payload\":{\"rate_limits\":null}}\n",
            ),
        )
        .unwrap();
        let w = codex_usage(&home).unwrap();
        std::fs::remove_dir_all(&home).ok();
        assert_eq!(w[0]["label"], "5h");
        assert_eq!(w[0]["resets_at"], 100_000);
        assert_eq!(w[1]["label"], "Semana");
        assert_eq!(w[1]["pct"], 40.0);
        assert!(codex_usage(Path::new("/nao/existe")).is_err());
    }

    #[test]
    fn status_do_pc_tem_valores_validos() {
        let v = tauri::async_runtime::block_on(pc_status()).unwrap();
        println!("{v}");
        for k in ["cpu", "ram", "disk"] {
            let x = v[k].as_f64().unwrap();
            assert!((0.0..=100.0).contains(&x), "{k} = {x}");
        }
    }

    #[test]
    #[ignore = "usa rede e o login do gh"]
    fn github_responde() {
        let v = tauri::async_runtime::block_on(github_status()).unwrap();
        println!("{v}");
        assert!(v["open"].is_number());
    }
}
