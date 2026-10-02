// tunnelstat — лёгкий оверлей: скорость туннеля, стабильность, индикатор утечки.
//
// Данные берутся ТОЛЬКО из счётчиков ядра (GetIfTable2 / GetIpForwardTable).
// Сетевого трафика программа не создаёт вообще: ни пингов, ни проб, ни запросов.
// Конфиги VPN-клиентов не читаются и не меняются.

#![windows_subsystem = "windows"]
#![allow(unused_must_use)] // возвраты GDI/XInput нас здесь не интересуют

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

// ── состояния ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Ok,       // зелёный  — стабильно
    Warn,     // жёлтый   — нестабильно
    Degraded, // оранжевый — обрыв / потери
    Leak,     // алый     — трафик идёт мимо VPN
    NoTunnel, // серый    — VPN не поднят
}

impl State {
    /// 0x00RRGGBB
    fn rgb(self) -> u32 {
        match self {
            State::Ok => 0x3DD68C,
            State::Warn => 0xE9C46A,
            State::Degraded => 0xF0883E,
            State::Leak => 0xF2545B,
            State::NoTunnel => 0x6B7280,
        }
    }
    fn label(self) -> &'static str {
        match self {
            State::Ok => "stable",
            State::Warn => "unstable",
            State::Degraded => "dropped",
            State::Leak => "leak: traffic bypassing VPN",
            State::NoTunnel => "VPN is down",
        }
    }
}

fn bgra(rgb: u32) -> u32 {
    (rgb & 0xFF) << 16 | (rgb & 0xFF00) | (rgb >> 16)
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let f = |sh: u32| {
        let av = (a >> sh) & 0xFF;
        let bv = (b >> sh) & 0xFF;
        (av as f32 + (bv as f32 - av as f32) * t) as u32
    };
    f(16) | (f(8) << 8) | f(0)
}

// ── снимок сетевых интерфейсов ───────────────────────────────────────────────

#[derive(Clone)]
struct Snap {
    idx: u32,
    name: String,
    desc: String,
    if_type: u32,
    in_oct: u64,
    out_oct: u64,
    in_dis: u64,
    out_dis: u64,
    in_err: u64,
    out_err: u64,
    link_up: bool,
}

/// Alias/Description в MIB_IF_ROW2 — фиксированный буфер UTF-16, не PWSTR.
fn from_wbuf(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

unsafe fn read_interfaces() -> Vec<Snap> {
    let mut table: *mut MIB_IF_TABLE2 = ptr::null_mut();
    // WIN32_ERROR.NO_ERROR == 0
    if GetIfTable2(&mut table) != WIN32_ERROR(0) || table.is_null() {
        return Vec::new();
    }
    let hdr = &*table;
    // Table объявлен как [MIB_IF_ROW2; 1], реально там буфер на NumEntries записей.
    let rows = std::slice::from_raw_parts(hdr.Table.as_ptr(), hdr.NumEntries as usize);
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(Snap {
            idx: r.InterfaceIndex,
            name: from_wbuf(&r.Alias),
            desc: from_wbuf(&r.Description),
            if_type: r.Type,
            in_oct: r.InOctets,
            out_oct: r.OutOctets,
            in_dis: r.InDiscards,
            out_dis: r.OutDiscards,
            in_err: r.InErrors,
            out_err: r.OutErrors,
            link_up: r.ReceiveLinkSpeed + r.TransmitLinkSpeed > 0,
        });
    }
    FreeMibTable(table as *const c_void);
    out
}

/// Индекс интерфейса, держащего маршрут по умолчанию (IPv4, маска 0).
/// Метрики у VPN-туннеля и у Wi-Fi часто равны, поэтому предпочитаем `prefer`.
unsafe fn default_route_ifindex(prefer: Option<u32>) -> Option<u32> {
    let mut size: u32 = 0;
    // Вызов с nullptr — это запрос размера буфера. Он штатно возвращает
    // ERROR_INSUFFICIENT_BUFFER (122), это НЕ ошибка: важен заполненный size.
    let _ = GetIpForwardTable(None, &mut size, false);
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let tbl = buf.as_mut_ptr() as *mut MIB_IPFORWARDTABLE;
    if GetIpForwardTable(Some(tbl), &mut size, false) != 0 {
        return None;
    }
    let t = &*tbl;
    let mut routes: Vec<(u32, u32)> = Vec::new(); // (метрика, индекс)
    for i in 0..t.dwNumEntries as usize {
        let row = *t.table.as_ptr().add(i);
        if row.dwForwardMask != 0 {
            continue;
        }
        routes.push((row.dwForwardMetric1, row.dwForwardIfIndex));
    }
    if let Some(p) = prefer {
        if routes.iter().any(|(_, i)| *i == p) {
            return Some(p);
        }
    }
    routes.iter().min_by_key(|(m, _)| *m).map(|(_, i)| *i)
}

const TUNNEL_PATTERNS: &[&str] = &[
    "sing-box",
    "singbox",
    "hiddify",
    "clash",
    "mihomo",
    "wintun",
    "wireguard",
    "amnezia",
    "nekoray",
    "v2ray",
    "xray",
    "outline",
    "openvpn",
    "tun",
    "tap",
    "tailscale",
    "zerotier",
    "warp",
    "utun",
    "torsocks",
    "shadowsocks",
    "meta tunnel",
];

fn is_tunnel_by_name(s: &Snap) -> bool {
    if s.if_type == 131 {
        return true; // IF_TYPE_TUNNEL
    }
    let hay = format!("{} {}", s.name, s.desc).to_lowercase();
    TUNNEL_PATTERNS.iter().any(|p| hay.contains(p))
}

fn is_physical(s: &Snap) -> bool {
    s.if_type == 6 || s.if_type == 71 // Ethernet / IEEE80211
}

/// Loopback (IF_TYPE_SOFTWARE_LOOPBACK) туннелем быть не может.
fn is_loopback(s: &Snap) -> bool {
    s.if_type == 24
}

/// Совпадение имени адаптера с тем, что задал пользователь в конфиге.
fn name_matches(s: &Snap, want: &str) -> bool {
    let w = want.trim().to_lowercase();
    if w.is_empty() {
        return false;
    }
    let hay = format!("{} {}", s.name, s.desc).to_lowercase();
    hay == w || hay.contains(&w)
}

// ── состояние приложения ─────────────────────────────────────────────────────

/// Скорость на интерфейсе, байт/сек: (вниз, вверх)
type Rate = (f64, f64);

struct Gfx {
    // _dib / _old удерживают DIB и предыдущий объект — без них DC теряет битмап.
    _dib: HBITMAP,
    dc: HDC,
    bits: *mut u32,
    _old: HGDIOBJ,
    f_bold: HFONT,
    f_small: HFONT,
}

struct App {
    prev: HashMap<u32, Snap>,
    rates: HashMap<u32, Rate>,
    tunnel: Option<u32>,
    /// Адаптер, заданный пользователем вручную (None = автоопределение).
    pinned: Option<String>,
    samples: Vec<f64>,
    loss_score: u32,
    zero_ticks: u32,
    last_poll: std::time::Instant,
    down: f64,
    up: f64,
    phys_total: f64,
    gfx: Gfx,
    hwnd: HWND,
    pos: (i32, i32),
    size: (i32, i32),
    scale: f32,
    tick: u64,
    /// курсор над плашкой: показываем крестик и снимаем click-through
    hover: bool,
    zone: u32,
    interval: u32,
    cfg: Config,
    /// Принудительный показ состояний для скриншотов/README (--demo).
    demo: bool,
    /// Конкретное состояние 0..5 для детерминированных скриншотов.
    forced: Option<State>,
}

/// Строка для тултипа трея: имя туннеля + скорость + состояние.
///
/// ВАЖНО: NOTIFYICONDATA.szTip не поддерживает переносы строк на Windows 10/11,
/// переводы строк там отображаются мусором. Поэтому всё в одну строку.
fn tooltip(app: &App) -> String {
    let name = app
        .tunnel
        .and_then(|i| app.prev.get(&i))
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "no VPN".into());
    let s = format!(
        "tunnelstat | {} | down {} | up {} | {}",
        name,
        fmt_speed(app.down),
        fmt_speed(app.up),
        app.state().label()
    );
    // Некоторые треи режут по длине, поэтому следим за лимитом 127 символов.
    if s.chars().count() > 127 {
        s.chars().take(127).collect()
    } else {
        s
    }
}

/// Коэффициент вариации: чем больше, тем рванее скорость.
fn cv(samples: &[f64]) -> f64 {
    if samples.len() < 3 {
        return 0.0;
    }
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    if mean <= 1.0 {
        return 0.0;
    }
    let var = samples.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() / mean
}

impl App {
    fn poll(&mut self) {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_poll).as_secs_f64();
        self.last_poll = now;
        if dt <= 0.0 {
            return;
        }

        // После сна/гибернации счётчики прыгают: интервал огромный, и дельта
        // даст гигантский "всплеск скорости". Сбрасываем базу.
        if dt > 5.0 {
            self.prev.clear();
            self.rates.clear();
            self.samples.clear();
            self.loss_score = 0;
            self.zero_ticks = 0;
            self.down = 0.0;
            self.up = 0.0;
            self.phys_total = 0.0;
            self.tunnel = None;
        }

        let cur = unsafe { read_interfaces() };
        if cur.is_empty() {
            return;
        }

        // ── Выбор туннеля ──
        // Принцип для «любого пользователя»: туннель — это адаптер, который
        // держит маршрут по умолчанию и при этом НЕ физический. Так находятся
        // все TUN-клиенты (sing-box, xray, clash, wireguard, amnezia, outline,
        // v2rayN, ...) без списка имён. Список имён — запасной путь для
        // split-tunnel и клиентов, не ставящих маршрут по умолчанию.
        let def = unsafe { default_route_ifindex(self.tunnel) };

        if let Some(i) = self.tunnel {
            // Адаптер мог исчезнуть (переподключение VPN) или упасть.
            if !cur.iter().any(|s| s.idx == i && s.link_up) {
                self.tunnel = None;
                self.samples.clear();
                self.loss_score = 0;
                self.zero_ticks = 0;
            }
        }

        if self.tunnel.is_none() {
            // Явно заданный пользователем адаптер — главнее всего.
            let pinned = self.pinned.as_deref().and_then(|n| {
                cur.iter()
                    .find(|s| s.link_up && name_matches(s, n))
                    .map(|s| s.idx)
            });

            self.tunnel = pinned
                .or_else(|| {
                    // 1) Держит маршрут по умолчанию и не физический => почти наверняка VPN
                    def.and_then(|d| {
                        cur.iter()
                            .find(|s| s.idx == d && s.link_up && !is_physical(s))
                            .map(|s| s.idx)
                    })
                })
                .or_else(|| {
                    // 2) Известное имя адаптера
                    let c: Vec<&Snap> = cur
                        .iter()
                        .filter(|s| s.link_up && is_tunnel_by_name(s))
                        .collect();
                    c.iter()
                        .find(|s| Some(s.idx) == def)
                        .or_else(|| c.iter().max_by_key(|s| s.in_oct + s.out_oct))
                        .map(|s| s.idx)
                })
                .or_else(|| {
                    // 3) Нефизический адаптер с наибольшим трафиком
                    cur.iter()
                        .filter(|s| s.link_up && !is_physical(s) && !is_loopback(s))
                        .max_by_key(|s| s.in_oct + s.out_oct)
                        .map(|s| s.idx)
                });
        }

        // Дельты счётчиков → скорости по всем адаптерам.
        // Потери считаем только по туннелю: у выключенного клиента счётчики
        // ошибок застывшие, иначе индикатор вечно показывал бы «обрыв».
        self.rates.clear();
        let mut lost: u64 = 0;
        for s in &cur {
            if let Some(p) = self.prev.get(&s.idx) {
                self.rates.insert(
                    s.idx,
                    (
                        s.in_oct.saturating_sub(p.in_oct) as f64 / dt,
                        s.out_oct.saturating_sub(p.out_oct) as f64 / dt,
                    ),
                );
                if Some(s.idx) == self.tunnel {
                    lost += s.in_dis.saturating_sub(p.in_dis)
                        + s.out_dis.saturating_sub(p.out_dis)
                        + s.in_err.saturating_sub(p.in_err)
                        + s.out_err.saturating_sub(p.out_err);
                }
            }
        }
        // Счётчик «нестабильности»: растёт при потерях, тает по секунде без потерь.
        // Так единичный сбой не держит жёлтый вечно, а серия сбоев копит оранжевый.
        self.loss_score = if lost > 0 {
            (self.loss_score + 1).min(10)
        } else {
            self.loss_score.saturating_sub(1)
        };

        let (d, u) = self
            .tunnel
            .and_then(|i| self.rates.get(&i))
            .copied()
            .unwrap_or((0.0, 0.0));
        self.down = d;
        self.up = u;

        // При исправном VPN по физическому адаптеру идёт сам зашифрованный туннель,
        // поэтому сравниваем его с трафиком туннеля, а не считаем утечкой сам по себе.
        self.phys_total = cur
            .iter()
            .filter(|s| is_physical(s) && Some(s.idx) != self.tunnel)
            .filter_map(|s| self.rates.get(&s.idx))
            .map(|(d, u)| d + u)
            .sum();

        // Сколько секунд подряд через туннель не идёт ни байта.
        if d + u < 1024.0 {
            // Cap it: at 1 Hz an u32 would wrap after ~49 days of a connected but
            // idle tunnel, and the wrap would read as "traffic flowing again".
            self.zero_ticks = self.zero_ticks.saturating_add(1).min(100_000);
        } else {
            self.zero_ticks = 0;
        }
        // Окно стабильности: только активные семплы, иначе CV взрывается на простое.
        if d + u > 2048.0 {
            self.samples.push(d);
            if self.samples.len() > 30 {
                self.samples.remove(0);
            }
        } else if !self.samples.is_empty() {
            self.samples.remove(0);
        }

        self.prev = cur.into_iter().map(|s| (s.idx, s)).collect();
        self.render();
    }

    fn state(&self) -> State {
        // --state N: одно конкретное состояние, для детерминированных скриншотов
        if let Some(s) = self.forced {
            return s;
        }
        // --demo: показываем состояния по кругу для самопроверки
        if self.demo {
            const CYCLE: [State; 5] = [
                State::Ok,
                State::Warn,
                State::Degraded,
                State::Leak,
                State::NoTunnel,
            ];
            let idx = ((self.tick / 6) as usize) % CYCLE.len();
            return CYCLE[idx];
        }
        let tun = match self.tunnel {
            None => return State::NoTunnel,
            Some(i) => i,
        };
        let up = self.prev.get(&tun).is_some_and(|s| s.link_up);

        if !up {
            // адаптер туннеля есть, но линк не поднят — соединение рухнуло
            return State::Degraded;
        }

        // ── Красный: только доказуемые факты, без эвристик ──
        //
        // (A) туннель поднят, но НЕ держит маршрут по умолчанию ⇒ трафик
        //     физически не идёт через него.
        if unsafe { default_route_ifindex(Some(tun)) } != Some(tun) {
            return State::Leak;
        }
        // (B) туннель держит маршрут, но через него 5+ секунд не идёт ни байта,
        //     тогда как физический адаптер активно грузится ⇒ трафик в обход.
        if self.zero_ticks >= 5 && self.phys_total > 32.0 * 1024.0 {
            return State::Leak;
        }

        // ── Оранжевый: реальные потери или обрыв ──
        // Локально качество канала (latency/jitter) не измерить без сетевых проб,
        // а разброс скорости отражает характер твоего трафика, а не сеть.
        // Поэтому оранжевый = только discards/errors на туннеле или обрыв линка.
        if self.loss_score >= 4 {
            return State::Degraded;
        }
        // ── Жёлтый: потери были, либо рваная скорость ──
        if self.loss_score >= 1 {
            return State::Warn;
        }
        let j = cv(&self.samples);
        if self.samples.len() >= 25 && j >= 2.0 {
            return State::Warn;
        }
        State::Ok
    }

    // ── отрисовка (GDI + ручная альфа-маска) ────────────────────────────────

    fn render(&mut self) {
        self.tick += 1;
        if self.tick % 5 == 1 {
            let st = self.state();
            let tname = self
                .tunnel
                .and_then(|i| self.prev.get(&i))
                .map(|s| format!("{}({})", s.name, s.idx))
                .unwrap_or_else(|| "нет".into());
            let cvs = cv(&self.samples);
            diag(&format!(
                "tun={} down={:.0} up={:.0} phys={:.0} ratio={:.2} loss={} n={} cv={:.3} => {:?}",
                tname,
                self.down,
                self.up,
                self.phys_total,
                self.phys_total / (self.down + self.up).max(1.0),
                self.loss_score,
                self.samples.len(),
                cvs,
                st
            ));
        }
        self.draw();
        // Тултип трея обновляем редко (раз в ~10 с): Shell_NotifyIcon не бесплатен,
        // но пользователь должен видеть актуальную скорость без наведения на панель.
        if self.tick % 10 == 0 {
            unsafe { tray_tip(self.hwnd, &tooltip(self)) };
        }
    }

    /// Пересоздать буфер и шрифты под новый масштаб (смена DPI/монитора).
    fn rebuild_gfx(&mut self) {
        unsafe {
            let g = make_gfx(self.size.0, self.size.1, self.scale);
            // Старое освобождаем, иначе утечка GDI при каждой смене монитора.
            let old_dc = self.gfx.dc;
            let old_dib = self.gfx._dib;
            let old_obj = self.gfx._old;
            DeleteDC(old_dc);
            DeleteObject(HGDIOBJ(old_dib.0));
            if !old_obj.0.is_null() {
                DeleteObject(old_obj);
            }
            DeleteObject(HGDIOBJ(self.gfx.f_bold.0));
            DeleteObject(HGDIOBJ(self.gfx.f_small.0));
            self.gfx = g;
        }
    }

    fn draw(&mut self) {
        unsafe {
            let (w, h) = self.size;
            let s = self.scale;
            let st = self.state();
            let col = st.rgb();
            let bg = 0x181B22u32;

            let dc = self.gfx.dc;

            // 1. фон на весь буфер
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            let bbrush = CreateSolidBrush(COLORREF(bgra(bg)));
            FillRect(dc, &rc, bbrush);
            DeleteObject(HGDIOBJ(bbrush.0));

            // 2. рамка
            let inset = (0.5 * s) as i32;
            rc = RECT {
                left: inset,
                top: inset,
                right: w - inset,
                bottom: h - inset,
            };
            let pen = CreatePen(PS_SOLID, 1, COLORREF(bgra(0x3E4A60)));
            let old = SelectObject(dc, HGDIOBJ(pen.0));
            let hollow = GetStockObject(NULL_BRUSH);
            let oldb = SelectObject(dc, hollow);
            RoundRect(
                dc,
                rc.left,
                rc.top,
                rc.right,
                rc.bottom,
                (26.0 * s) as i32,
                (26.0 * s) as i32,
            );
            SelectObject(dc, oldb);
            SelectObject(dc, old);
            DeleteObject(HGDIOBJ(pen.0));

            // 3. индикатор
            let cx = (19.0 * s) as i32;
            let cy = h / 2;
            if matches!(st, State::Leak | State::Degraded) {
                // мягкое гало из нескольких колец, сведённых к цвету фона
                for (rr, t) in [(14.0f32, 0.62f32), (11.5, 0.38), (9.5, 0.2)] {
                    let c = lerp_rgb(bg, col, t);
                    let b = CreateSolidBrush(COLORREF(bgra(c)));
                    let r = (rr * s) as i32;
                    let old = SelectObject(dc, HGDIOBJ(b.0));
                    let oldp = SelectObject(dc, GetStockObject(NULL_PEN));
                    Ellipse(dc, cx - r, cy - r, cx + r, cy + r);
                    SelectObject(dc, oldp);
                    SelectObject(dc, old);
                    DeleteObject(HGDIOBJ(b.0));
                }
            }
            let dbr = (6.0 * s) as i32;
            let dot = CreateSolidBrush(COLORREF(bgra(col)));
            let old = SelectObject(dc, HGDIOBJ(dot.0));
            let oldp = SelectObject(dc, GetStockObject(NULL_PEN));
            Ellipse(dc, cx - dbr, cy - dbr, cx + dbr, cy + dbr);
            SelectObject(dc, oldp);
            SelectObject(dc, old);
            DeleteObject(HGDIOBJ(dot.0));

            // 4. текст
            SetBkMode(dc, TRANSPARENT);
            let tx = (33.0 * s) as i32;
            // справа резервируем место под крестик, чтобы текст под ним не лез
            let tw = w - tx - (32.0 * s) as i32;

            SetTextColor(dc, COLORREF(bgra(0xE9ECF2)));
            let old = SelectObject(dc, HGDIOBJ(self.gfx.f_bold.0));
            let mut l1 = wide(&format!(
                "↓ {}   ↑ {}",
                fmt_speed(self.down),
                fmt_speed(self.up)
            ));
            let mut r1 = RECT {
                left: tx,
                top: (10.0 * s) as i32,
                right: tx + tw,
                bottom: (35.0 * s) as i32,
            };
            DrawTextW(
                dc,
                &mut l1,
                &mut r1,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            SelectObject(dc, old);

            SetTextColor(dc, COLORREF(bgra(col)));
            let old = SelectObject(dc, HGDIOBJ(self.gfx.f_small.0));
            let mut l2 = wide(st.label());
            let mut r2 = RECT {
                left: tx,
                top: (36.0 * s) as i32,
                right: tx + tw,
                bottom: (58.0 * s) as i32,
            };
            DrawTextW(
                dc,
                &mut l2,
                &mut r2,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            SelectObject(dc, old);

            // 4b. крестик — только когда курсор над плашкой
            if self.hover {
                let mid = close_center(w, h, s);
                let pen = CreatePen(PS_SOLID, (1.6 * s) as i32, COLORREF(bgra(0xC9D1E0)));
                let old = SelectObject(dc, HGDIOBJ(pen.0));
                let r = (5.5 * s) as i32;
                MoveToEx(dc, mid.0 - r, mid.1 - r, None);
                LineTo(dc, mid.0 + r, mid.1 + r);
                MoveToEx(dc, mid.0 + r, mid.1 - r, None);
                LineTo(dc, mid.0 - r, mid.1 + r);
                SelectObject(dc, old);
                DeleteObject(HGDIOBJ(pen.0));
            }

            // 5. альфа-канал по SDF скруглённого прямоугольника + премножилизация
            apply_alpha(self.gfx.bits, w, h, 13.0 * s);

            // 6. на экран
            // Позицию берём из реального окна, а не из self.pos: UpdateLayeredWindow
            // ПЕРЕМЕЩАЕТ окно, поэтому устаревшая self.pos откатывала бы
            // перетаскивание назад при каждой перерисовке.
            let mut wr = RECT::default();
            GetWindowRect(self.hwnd, &mut wr);
            self.pos = (wr.left, wr.top);
            let screen = GetDC(None);
            let dst = POINT {
                x: wr.left,
                y: wr.top,
            };
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let bf = BLENDFUNCTION {
                BlendOp: 0,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: 1,
            };
            UpdateLayeredWindow(
                self.hwnd,
                screen,
                Some(&dst),
                Some(&size),
                dc,
                Some(&src),
                COLORREF(0),
                Some(&bf),
                ULW_ALPHA,
            )
            .ok();
            ReleaseDC(None, screen);
        }
    }
}

/// Знаковое расстояние до скруглённого прямоугольника (для покрытия краёв).
fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy) - r
}

unsafe fn apply_alpha(bits: *mut u32, w: i32, h: i32, r: f32) {
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let hw = w as f32 / 2.0;
    let hh = h as f32 / 2.0;
    for y in 0..h {
        let row = bits.offset((y * w) as isize);
        for x in 0..w {
            // сглаживание на ширину ~1 пикселя
            let d = sd_round_rect(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw, hh, r);
            let a = (0.5 - d).clamp(0.0, 1.0);
            let p = row.offset(x as isize);
            if a >= 1.0 {
                *p |= 0xFF00_0000;
            } else if a <= 0.0 {
                *p &= 0x00FF_FFFF;
            } else {
                let c = *p & 0x00FF_FFFF;
                let f = (a * 255.0) as u32;
                *p = ((c & 0xFF) * f / 255)
                    | ((((c >> 8) & 0xFF) * f / 255) << 8)
                    | ((((c >> 16) & 0xFF) * f / 255) << 16)
                    | (f << 24);
            }
        }
    }
}

fn fmt_speed(b: f64) -> String {
    const K: f64 = 1024.0;
    const U: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = b.max(0.0);
    let mut i = 0;
    while v >= 999.5 && i < 3 {
        v /= K;
        i += 1;
    }
    let dec = if v >= 100.0 {
        0
    } else if v >= 10.0 {
        1
    } else {
        2
    };
    format!("{:.dec$} {}", v, U[i], dec = dec)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Диагностика в tunnelstat.log рядом с exe. Только при запуске с --debug.
fn diag(msg: &str) {
    if std::env::var("TUNNELSTAT_DEBUG").is_err() {
        return;
    }
    use std::io::Write;
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("tunnelstat.log")));
    if let Some(path) = path {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{}", msg);
        }
    }
}

// ── создание ресурсов ────────────────────────────────────────────────────────

unsafe fn make_gfx(w: i32, h: i32, s: f32) -> Gfx {
    let screen = GetDC(None);
    let dc = CreateCompatibleDC(screen);
    ReleaseDC(None, screen);

    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;

    let mut bits: *mut c_void = ptr::null_mut();
    let dib = CreateDIBSection(dc, &bmi as *const _, DIB_RGB_COLORS, &mut bits, None, 0)
        .unwrap_or_default();
    let old = SelectObject(dc, HGDIOBJ(dib.0));

    let face = wide("Segoe UI");
    // Высота шрифта передаётся ОТРИЦАТЕЛЬНОЙ (это «высота символа», а не строка).
    // Поэтому max(1) здесь недопустим: -34.max(1) == 1, и шрифт схлопывался
    // до одного пикселя. Защищаем только ноль.
    let h_big = -((17.0 * s) as i32);
    let h_small = -((13.0 * s) as i32);
    // (высота, ширина, escapement, orientation, weight, italic, underline,
    //  strikeout, charset, outprec, clipprec, quality, pitchandfamily)
    let f_bold = CreateFontW(
        if h_big == 0 { -1 } else { h_big },
        0,
        0,
        0,
        700,
        0,
        0,
        0,
        1,
        0,
        0,
        4,
        0x22,
        PCWSTR(face.as_ptr()),
    );
    let f_small = CreateFontW(
        if h_small == 0 { -1 } else { h_small },
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        1,
        0,
        0,
        4,
        0x22,
        PCWSTR(face.as_ptr()),
    );

    Gfx {
        _dib: dib,
        dc,
        bits: bits as *mut u32,
        _old: old,
        f_bold,
        f_small,
    }
}

// ── позиция: экран делим 3×2, нумерация снизу вверх по колонкам ───────────────

/// Центр крестика в координатах окна (правый верхний угол).
fn close_center(w: i32, _h: i32, s: f32) -> (i32, i32) {
    ((w as f32 - 19.0 * s) as i32, (19.0 * s) as i32)
}

/// Прямоугольник крестика в экранных координатах — для WM_NCHITTEST.
fn close_hit(screen: POINT, w: i32, h: i32, s: f32) -> RECT {
    let c = close_center(w, h, s);
    let pad = (11.0 * s) as i32;
    RECT {
        left: screen.x + c.0 - pad,
        top: screen.y + c.1 - pad,
        right: screen.x + c.0 + pad,
        bottom: screen.y + c.1 + pad,
    }
}

fn cfg_file(name: &str) -> Option<std::path::PathBuf> {
    let base = std::env::var("APPDATA").ok()?;
    Some(std::path::Path::new(&base).join("tunnelstat").join(name))
}

fn load_pos() -> Option<(i32, i32)> {
    let p = cfg_file("pos.txt")?;
    let t = std::fs::read_to_string(p).ok()?;
    let mut it = t.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

fn save_pos(pos: (i32, i32)) {
    if let Some(p) = cfg_file("pos.txt") {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, format!("{} {}", pos.0, pos.1));
    }
}

/// Настройки из `%APPDATA%\tunnelstat\config.txt`. Необязательный файл:
/// без него всё работает на автоопределении.
#[derive(Default)]
struct Config {
    zone: Option<u32>,
    interval: Option<u32>,
    /// Имя адаптера, если автоопределение не подошло.
    interface: Option<String>,
}

fn load_config() -> Config {
    let mut c = Config::default();
    let p = match cfg_file("config.txt") {
        Some(p) => p,
        None => return c,
    };
    let text = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(_) => return c,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let mut kv = line.splitn(2, '=');
        let k = kv.next().unwrap_or("").trim().to_lowercase();
        let v = kv.next().unwrap_or("").trim().to_string();
        match k.as_str() {
            "zone" => c.zone = v.parse().ok(),
            "interval" => c.interval = v.parse().ok(),
            "interface" if !v.is_empty() => c.interface = Some(v),
            _ => {}
        }
    }
    c
}

fn save_config(c: &Config, zone: u32, interval: u32) {
    if let Some(p) = cfg_file("config.txt") {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut t = String::from("# tunnelstat settings\n");
        t.push_str(&format!("zone={}\n", zone));
        t.push_str(&format!("interval={}\n", interval));
        t.push_str(&format!(
            "interface={}\n",
            c.interface.as_deref().unwrap_or("")
        ));
        let _ = std::fs::write(p, t);
    }
}

fn work_area() -> (i32, i32, i32, i32) {
    let mut wa = RECT::default();
    unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    (wa.left, wa.top, wa.right, wa.bottom)
}

fn compute_pos(zone: u32, size: (i32, i32)) -> (i32, i32) {
    let (wl, wt, wr, wb) = work_area();
    let (ww, wh) = (wr - wl, wb - wt);
    let z = zone.clamp(1, 6);
    let col = ((z - 1) / 2) as f32;
    let rowb = ((z - 1) % 2) as f32;
    let cx = wl as f32 + (col + 0.5) * (ww as f32 / 3.0);
    let cy = wb as f32 - (rowb + 0.5) * (wh as f32 / 2.0);
    let mut x = (cx - size.0 as f32 / 2.0) as i32;
    let mut y = (cy - size.1 as f32 / 2.0) as i32;
    x = x.clamp(wl + 4, wl + ww - size.0 - 4);
    y = y.clamp(wt + 4, wt + wh - size.1 - 4);
    (x, y)
}

// ── Win32 ────────────────────────────────────────────────────────────────────

static mut APP: *mut App = ptr::null_mut();

/// Снимает/возвращает WS_EX_TRANSPARENT, чтобы плашка была кликабельной
/// только под курсором и не мешала работе в остальное время.
unsafe fn set_interactive(hwnd: HWND, on: bool) {
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let want = if on {
        ex & !WS_EX_TRANSPARENT.0
    } else {
        ex | WS_EX_TRANSPARENT.0
    };
    if want == ex {
        return;
    }
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want as isize);
    // после смены стилей окно нужно пересоздать, иначе hit-test не обновится
    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
}

/// Опрашивает позицию курсора: над плашкой или нет.
fn update_hover(app: &mut App) {
    let mut p = POINT::default();
    let ok = unsafe { GetCursorPos(&mut p) }.is_ok();
    let mut r = RECT::default();
    unsafe { GetWindowRect(app.hwnd, &mut r) };
    let pad = (6.0 * app.scale) as i32;
    let inside = ok
        && p.x >= r.left - pad
        && p.x <= r.right + pad
        && p.y >= r.top - pad
        && p.y <= r.bottom + pad;

    if inside == app.hover {
        return;
    }
    app.hover = inside;
    unsafe { set_interactive(app.hwnd, inside) };
    app.render();
}

// ── трей ─────────────────────────────────────────────────────────────────────
// Без трея выход неочевиден: горячие клавиши могут быть заняты другим ПО.

const ID_TRAY: u32 = 0x564E5354; // "VNST"
const WM_TRAY: u32 = WM_APP + 1;

unsafe fn tray_add(hwnd: HWND) -> bool {
    let hinst = HINSTANCE(hwnd.0);
    let icon = LoadIconW(hinst, PCWSTR(1usize as *const u16)).unwrap_or_default();
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ID_TRAY,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    let tip = wide("tunnelstat");
    let n = tip.len().min(d.szTip.len() - 1);
    d.szTip[..n].copy_from_slice(&tip[..n]);
    Shell_NotifyIconW(NIM_ADD, &d).as_bool()
}

unsafe fn tray_tip(hwnd: HWND, text: &str) {
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ID_TRAY,
        uFlags: NIF_TIP,
        ..Default::default()
    };
    let t = wide(text);
    let n = t.len().min(d.szTip.len() - 1);
    d.szTip[..n].copy_from_slice(&t[..n]);
    Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe fn tray_del(hwnd: HWND) {
    let d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ID_TRAY,
        ..Default::default()
    };
    Shell_NotifyIconW(NIM_DELETE, &d);
}

/// Меню трея: показать/скрыть, зоны, выход.
unsafe fn tray_menu(hwnd: HWND) {
    let h = match CreatePopupMenu() {
        Ok(h) => h,
        Err(_) => return,
    };
    let mut sid = 10usize;
    let mut zone_items: Vec<(u32, usize)> = Vec::new(); // (zone, id)

    let b_hide = wide("Hide overlay");
    let b_quit = wide("Quit");
    let _ = AppendMenuW(h, MF_STRING, sid, PCWSTR(b_hide.as_ptr()));
    let id_hide = sid;
    sid += 1;

    let _ = AppendMenuW(h, MF_SEPARATOR, 0, PCWSTR::null());

    for z in 1..=6u32 {
        let buf = wide(&format!(
            "Zone {}  (col {}, row {})",
            z,
            (z - 1) / 2,
            (z - 1) % 2
        ));
        let _ = AppendMenuW(h, MF_STRING, sid, PCWSTR(buf.as_ptr()));
        zone_items.push((z, sid));
        sid += 1;
    }

    let _ = AppendMenuW(h, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(h, MF_STRING, sid, PCWSTR(b_quit.as_ptr()));
    let id_quit = sid;

    // Чтобы меню закрывалось по клику мышью, окно должно быть foreground.
    let mut p = POINT::default();
    let _ = GetCursorPos(&mut p);
    SetForegroundWindow(hwnd);
    // С TPM_RETURNCMD выбранный id возвращается прямо в результате, без WM_COMMAND.
    let cmd =
        TrackPopupMenu(h, TPM_RIGHTBUTTON | TPM_RETURNCMD, p.x, p.y, 0, hwnd, None).0 as usize;
    let _ = DestroyMenu(h);

    if cmd == id_hide {
        ShowWindow(hwnd, SW_HIDE);
    } else if cmd == id_quit {
        DestroyWindow(hwnd);
    } else if cmd != 0 {
        let app = &mut *APP;
        for (z, id) in zone_items {
            if cmd == id {
                app.pos = compute_pos(z, app.size);
                app.zone = z;
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    app.pos.0,
                    app.pos.1,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
                save_pos(app.pos);
                save_config(&app.cfg, app.zone, app.interval);
                app.render();
                break;
            }
        }
    }
    // После закрытия меню окно теряет foreground — возвращаем фокус.
    PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let app = &mut *APP;
    match msg {
        WM_TIMER => {
            match wp.0 {
                1 => app.poll(),        // статистика, раз в секунду
                _ => update_hover(app), // наведение, 16 раз в секунду
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            if !app.hover {
                return LRESULT(HTTRANSPARENT as isize);
            }
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let hr = close_hit(POINT { x, y }, app.size.0, app.size.1, app.scale);
            let p = POINT { x, y };
            if PtInRect(&hr, p).as_bool() {
                LRESULT(HTCLOSE as isize)
            } else {
                // HTCAPTION даёт перетаскивание мышью без своего кода
                LRESULT(HTCAPTION as isize)
            }
        }
        WM_EXITSIZEMOVE => {
            // мышь отпущена после перетаскивания — запоминаем место
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r);
            app.pos = (r.left, r.top);
            save_pos(app.pos);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // курсор ушёл с плашки во время перетаскивания — не сбрасываем hover
            LRESULT(0)
        }
        WM_CREATE => {
            // Иконка из ресурса PE (id = 1). WS_EX_NOACTIVATE этому не мешает.
            let hinst = HINSTANCE(hwnd.0);
            if let Ok(big) = LoadIconW(hinst, PCWSTR(1usize as *const u16)) {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    WPARAM(ICON_SMALL as usize),
                    LPARAM(big.0 as isize),
                );
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            // Сменилось разрешение, монитор или масштаб: пересчитываем геометрию.
            // Иначе на ноутбуке после подключения внешнего монитора плашка уезжает.
            let dpi = if GetDpiForSystem() > 0 {
                GetDpiForSystem()
            } else {
                96
            };
            let ns = dpi as f32 / 96.0;
            let new_size = ((280.0 * ns) as i32, (66.0 * ns) as i32);
            if new_size != app.size {
                app.size = new_size;
                app.scale = ns;
                // шрифты и буфер зависят от масштаба — пересоздаём
                app.rebuild_gfx();
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    app.pos.0,
                    app.pos.1,
                    new_size.0,
                    new_size.1,
                    SWP_NOACTIVATE,
                );
            }
            app.render();
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            // Изменилась рабочая область (панель задач, разрешение)
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r);
            app.pos = (r.left, r.top);
            LRESULT(0)
        }
        WM_TRAY => match lp.0 as u32 {
            WM_LBUTTONDBLCLK => {
                let vis = IsWindowVisible(hwnd).as_bool();
                ShowWindow(hwnd, if vis { SW_HIDE } else { SW_SHOWNA });
                LRESULT(0)
            }
            WM_CONTEXTMENU => {
                tray_menu(hwnd);
                LRESULT(0)
            }
            _ => LRESULT(0),
        },
        WM_HOTKEY => match wp.0 as u32 {
            1 => {
                // Ctrl+Alt+V — показать/скрыть
                let vis = IsWindowVisible(hwnd).as_bool();
                ShowWindow(hwnd, if vis { SW_HIDE } else { SW_SHOWNA });
                LRESULT(0)
            }
            2 => {
                // Ctrl+Alt+Q — выход
                DestroyWindow(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        },
        WM_CLOSE => {
            DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            tray_del(hwnd);
            save_pos(app.pos);
            save_config(&app.cfg, app.zone, app.interval);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn main() {
    unsafe {
        let args: Vec<String> = std::env::args().collect();
        // Приоритет: CLI > config.txt > значения по умолчанию
        let cfg = load_config();
        let mut zone = cfg.zone.unwrap_or(5);
        let mut interval = cfg.interval.unwrap_or(1000).max(200);
        let mut demo = false;
        let mut forced: Option<State> = None;
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--demo" => {
                    demo = true;
                    i += 1;
                }
                "--state" if i + 1 < args.len() => {
                    const ALL: [State; 5] = [
                        State::Ok,
                        State::Warn,
                        State::Degraded,
                        State::Leak,
                        State::NoTunnel,
                    ];
                    let n: usize = args[i + 1].parse().unwrap_or(0);
                    if n < ALL.len() {
                        forced = Some(ALL[n]);
                    }
                    i += 2;
                }
                "--zone" if i + 1 < args.len() => {
                    zone = args[i + 1].parse().unwrap_or(5).clamp(1, 6);
                    i += 2;
                }
                "--interval" if i + 1 < args.len() => {
                    interval = args[i + 1].parse().unwrap_or(1000).max(200);
                    i += 2;
                }
                "--reset" => {
                    // сбросить сохранённое положение
                    if let Some(p) = cfg_file("pos.txt") {
                        let _ = std::fs::remove_file(p);
                    }
                    i += 1;
                }
                "--help" | "-h" => {
                    println!("tunnelstat - VPN tunnel speed and health overlay");
                    println!();
                    println!("Usage: tunnelstat.exe [options]");
                    println!();
                    println!("  --zone 1..6     screen cell of a 3x2 grid, numbered bottom-up");
                    println!("                  per column (5 = bottom of the right column)");
                    println!("  --interval MS   poll interval, min 200 (default 1000);");
                    println!("                  lower = more responsive, more CPU");
                    println!("  --reset         forget the saved panel position");
                    println!("  --state 0..4    pin one state: 0 stable, 1 unstable, 2 dropped,");
                    println!("                  3 leak, 4 VPN down (for screenshots)");
                    println!("  --demo          cycle all states (for screenshots)");
                    println!("  -h, --help      this text");
                    println!();
                    println!(
                        "Config: %APPDATA%\\tunnelstat\\config.txt  (zone, interval, interface)"
                    );
                    println!("Tray:   click toggles the panel, right-click opens the menu");
                    println!("Keys:   Ctrl+Alt+V toggle, Ctrl+Alt+Q quit");
                    println!("        TUNNELSTAT_DEBUG=1 writes tunnelstat.log next to the exe");
                    return;
                }
                _ => i += 1,
            }
        }

        let mname = wide("Local\\tunnelstat_single");
        let _mutex = CreateMutexW(None, true, PCWSTR(mname.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let screen = GetDC(None);
        // Манифест объявляет DPI-awareness до старта процесса, поэтому API-вызов
        // вернёт ACCESS_DENIED — это НЕ ошибка, а признак того, что манифест
        // сработал. Ориентируемся на реальный dpi, а не на результат вызова.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let dpi = GetDpiForSystem();
        ReleaseDC(None, screen);
        let s = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        let wa = work_area();
        diag(&format!(
            "dpi={} scale={:.2} wa=({},{})-({},{})",
            dpi, s, wa.0, wa.1, wa.2, wa.3
        ));

        // panic="abort" в release: unwrap здесь означал бы смерть процесса.
        // GetModuleHandleW(NULL) для запущенного exe не падает, но и полагаться
        // на это не стоит — при неудаче просто тихо выходим.
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => HINSTANCE(h.0),
            Err(_) => return,
        };
        let cname = wide("tunnelstat_overlay");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0x0002 | 0x0001),
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst,
            lpszClassName: PCWSTR(cname.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let size = ((280.0 * s) as i32, (66.0 * s) as i32);
        let pos = load_pos().unwrap_or_else(|| compute_pos(zone, size));

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            PCWSTR(cname.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            pos.0,
            pos.1,
            size.0,
            size.1,
            None,
            None,
            hinst,
            None,
        )
        .unwrap_or_default();

        // Горячие клавиши могут быть заняты другим ПО — это не повод молча
        // ломаться. В трее всегда есть доступ к выходу.
        let mods = MOD_ALT | MOD_CONTROL | MOD_NOREPEAT;
        let hk_v = RegisterHotKey(hwnd, 1, mods, 0x56);
        let hk_q = RegisterHotKey(hwnd, 2, mods, 0x51);
        if hk_v.is_err() || hk_q.is_err() {
            diag("hotkey registration failed; tray menu remains the way out");
        }

        tray_add(hwnd);
        // Пишем конфиг сразу: файл должен существовать, чтобы пользователь мог
        // его найти и отредактировать, даже если позже закроет процесс kill'ом.
        save_config(&cfg, zone, interval);

        let gfx = make_gfx(size.0, size.1, s);
        let app = Box::new(App {
            prev: HashMap::new(),
            rates: HashMap::new(),
            tunnel: None,
            pinned: cfg.interface.clone(),
            samples: Vec::new(),
            loss_score: 0,
            zero_ticks: 0,
            last_poll: std::time::Instant::now(),
            down: 0.0,
            up: 0.0,
            phys_total: 0.0,
            gfx,
            hwnd,
            pos,
            size,
            scale: s,
            tick: 0,
            hover: false,
            zone,
            interval,
            cfg,
            demo,
            forced,
        });
        APP = Box::into_raw(app);

        // два холостых опроса, чтобы посчитать первую дельту
        (*APP).poll();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        (*APP).poll();
        // тултип трея сразу показывает состояние
        tray_tip(hwnd, &tooltip(&(*APP)));

        SetTimer(hwnd, 1, interval, None);
        // таймер наведения: 60 мс, чтобы крестик появлялся мгновенно
        SetTimer(hwnd, 2, 60, None);
        ShowWindow(hwnd, SW_SHOWNA);

        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0).0;
            if r <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
