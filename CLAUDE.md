# SmartScreen

App desktop (Tauri v2) para configurar um relógio/tela Wi‑Fi "Smart Weather Clock" com firmware SD PRO
(https://github.com/JUZIPi-tech/SD_PRO — só binários `.bin` e manual, sem código-fonte).
O app substitui a página web da própria tela. UI em português.

## Comandos

```sh
pnpm install          # usar pnpm, não npm
pnpm dev              # roda o app (recompila o Rust e reabre a janela sozinho ao mudar src-tauri/)
pnpm build            # gera o instalador
cd src-tauri && cargo test
```

Dependências do sistema (CachyOS/Arch): `rustup` (toolchain stable), `webkit2gtk-4.1` e `libayatana-appindicator` (ícone da bandeja; sem ela o app não abre no Linux).

## Estrutura

- `ui/index.html` — toda a interface: HTML + CSS + JS num arquivo só, sem bundler nem framework.
  Visual pixel: prévia da tela à esquerda (`#screen`, aproximada — o firmware não expõe o layout real) e 4 abas.
- `ui/fonts/` — Silkscreen e VT323 (OFL) embutidas, porque o app roda offline.
- `src-tauri/src/main.rs` — registra o plugin HTTP e o comando `crop_gif`.
- `src-tauri/capabilities/default.json` — libera o plugin HTTP para `http://*` e para a API de Releases do GitHub.

## Aviso de versão nova

Ao abrir e a cada 6 h o app lê o último Release do GitHub (o CI publica um a cada PR mergeado na main) e,
se a tag for maior que a versão do app, mostra uma faixa com "Baixar" (abre o Release, plugin `opener`) e
uma notificação do sistema (plugin `notification`, uma vez por versão). Não instala sozinho.
O `tauri.conf.json` fica sempre em `0.1.0`: a versão real só é aplicada no CI. Por isso um build local
mostra o aviso sempre que existir Release mais novo que 0.1.0.

## Decisões que não são óbvias

- **Todo HTTP passa por `window.__TAURI__.http.fetch`** (tauri-plugin-http). A tela não envia cabeçalhos
  CORS, então o `fetch` normal do webview falha.
- **`crop_gif`** recebe os bytes crus do GIF (`invoke('crop_gif', Uint8Array, { headers: { size } })`),
  corta o centro em quadrado e redimensiona mantendo a animação. `size` = `80` (GIF do relógio, padrão)
  ou `240` (tela Photo). O webview não gera GIF, por isso é feito em Rust (crate `image`).
- Fotos comuns (JPG/PNG/WebP) são cortadas para 240×240 em JPEG 0.8 via canvas, no JS, igual à página original.
- `select` e `input[type=file]` usam `appearance: none` e estilo próprio: o WebKitGTK aplicava o tema GTK
  e o texto ficava ilegível no modo escuro.
- O IP da tela fica no `localStorage` (padrão `192.168.1.121`; em modo AP ela usa `192.168.4.1`).

## API da tela (HTTP, sem autenticação)

| Ação | Chamada |
|---|---|
| Ler config | `GET /config` |
| Mudar ajuste | `GET /api/set?key=<chave>&value=<valor>` |
| Temas | `GET /theme/list`, `/theme/toggle?id=N&state=0\|1`, `/theme/interval?val=<seg>` |
| GIF do relógio (80×80) | `POST /upload`, campo `imageFile`, nome fixo `4.gif` (igual à página original) |
| Fotos (240×240) | `GET /photo/list`, `POST /photo/upload` (campo `file`), `/photo/toggle?name=&state=`, `/photo/delete?name=`, `/photo/interval?val=` |
| Outros | `/restart`, `/scanwifi`, `POST /update_ota` (campo `update`) |

**As chaves de escrita diferem das de leitura** em alguns casos:

| Em `/config` | Em `/api/set` |
|---|---|
| `hour12` | `time12_24` |
| `brightness` | `lcd_brightness` |
| `sync` | `timesync` (+ `zone` = offset em horas) |
| `color1` (RGB565) | `color1` e também `ws_r`/`ws_g`/`ws_b` (RGB 0–255) |

Outras chaves: `city`, `celsius`, `mile`, `timeformat` (0 MDY, 1 DMY, 2 YMD), `gifnum` (1–4 prontos,
5 personalizado), `nightmode`, `starttime` (18–23), `stoptime` (5–12), `nightbrightness` (2–99), `ntp`,
`weatherkey`, `ssid`, `password`. Booleanos são enviados como `1`/`0`. Cores ficam em RGB565.

## Cuidados

- `GET /config` devolve a **senha do Wi‑Fi e a chave do OpenWeatherMap em texto puro**. Não registrar
  nem exibir essa resposta inteira.
- Não é possível criar telas novas no firmware. Para conteúdo próprio, gerar uma imagem 240×240 e
  enviar para a tela Photo. Cada envio regrava a flash da tela, então nada de envios a cada poucos segundos.
- O firmware original exige uma licença ligada ao MAC (manual). Trocar o firmware pode exigir pedir a licença de novo.
- Fora do app ficaram de propósito: Wi‑Fi, OTA, NTP e chave do OpenWeatherMap (estão na página da tela).
