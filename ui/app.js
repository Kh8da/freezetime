"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);
const main = $("main");

let S = null; // get_state
let H = null; // get_history, loaded when the history tab opens
let view = "session";
let openRecap = null; // a past session opened from history

/* ---------- helpers ---------- */

const ESC = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ESC[c]);

function errText(e) {
  const s = typeof e === "string" ? e : e?.message ?? String(e);
  return s || "something went wrong.";
}

let toastTimer = 0;
function toast(msg) {
  const t = $("toast");
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 2600);
}

/** "de_ancient" -> "Ancient", the way the game's own menus name maps. */
function mapName(name) {
  const base = String(name || "").split("/").pop();
  const short = base.replace(/^(de|cs)_/, "");
  return short ? short[0].toUpperCase() + short.slice(1) : "";
}

const MODES = { competitive: "competitive", premier: "premier", scrimcomp2v2: "wingman", casual: "casual", deathmatch: "deathmatch", gungameprogressive: "arms race" };
const modeName = (m) => MODES[m] ?? (m || "").replace(/_/g, " ");

const two = (n) => String(n).padStart(2, "0");
function clock(ts) {
  const d = new Date(ts * 1000);
  return `${two(d.getHours())}:${two(d.getMinutes())}`;
}
const DAYS = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
function day(ts) {
  const d = new Date(ts * 1000);
  const same = (a, b) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
  if (same(d, new Date())) return "today";
  if (same(d, new Date(Date.now() - 86400000))) return "yesterday";
  return `${DAYS[d.getDay()]} ${d.getDate()} ${MONTHS[d.getMonth()]}`;
}
function dur(secs) {
  const m = Math.max(0, Math.round(secs / 60));
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${two(m % 60)}m`;
}
const now = () => Math.floor(Date.now() / 1000);

function record(t) {
  return t.draws ? `${t.wins}-${t.losses}-${t.draws}` : `${t.wins}-${t.losses}`;
}

const ICON = {
  x: `<svg viewBox="0 0 16 16"><path d="M4 4l8 8M12 4l-8 8"/></svg>`,
  back: `<svg viewBox="0 0 16 16"><path d="M10 3.5L5.5 8l4.5 4.5"/></svg>`,
  chev: `<svg class="chev" viewBox="0 0 16 16"><path d="M6 3.5L10.5 8 6 12.5"/></svg>`,
};

/* ---------- data ---------- */

async function refresh() {
  try {
    S = await invoke("get_state");
  } catch (e) {
    toast(errText(e));
    return;
  }
  render();
}

async function loadHistory() {
  try {
    H = await invoke("get_history");
  } catch (e) {
    toast(errText(e));
  }
}

async function act(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    toast(errText(e));
    return undefined;
  }
}

/* ---------- shared pieces ---------- */

function matchRow(m, { showDay = false } = {}) {
  const r = m.outcome === "win" ? "W" : m.outcome === "loss" ? "L" : "D";
  const when = showDay ? `${day(m.ended)} ${clock(m.ended)}` : clock(m.ended);
  return `
    <div class="mrow">
      <div class="res ${m.outcome}" aria-label="${m.outcome}">${r}</div>
      <div class="info">
        <div class="name">${esc(mapName(m.map))}</div>
        <div class="sub">${esc(modeName(m.mode))} · ${dur(m.ended - m.started)}</div>
      </div>
      <div class="right">
        <div class="a">${m.score[0]}:${m.score[1]}</div>
        <div class="b"><span data-tip="${m.kills} kills, ${m.deaths} deaths, ${m.assists} assists">${m.kills}k ${m.deaths}d</span> · ${when}</div>
      </div>
    </div>`;
}

function mapTable(lines) {
  if (!lines.length) return "";
  const rows = lines
    .map((l) => {
      const rate = l.tally.matches ? Math.round((l.tally.wins * 100) / l.tally.matches) : 0;
      return `<tr>
        <td>${esc(mapName(l.map))}</td>
        <td>${record(l.tally)}</td>
        <td>${l.kd}</td>
        <td class="bar-cell"><div class="meter${rate < 50 ? " under" : ""}"><span style="width:${rate}%"></span></div></td>
      </tr>`;
    })
    .join("");
  return `<table class="table"><thead><tr><th></th><th>w-l</th><th>k/d</th><th class="bar-cell">win rate</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function tile(k, v, extra = "") {
  return `<div class="tile"><div class="k">${k}</div><div class="v${extra}">${v}</div></div>`;
}

/* ---------- session ---------- */

const PHASE = {
  warmup: "warmup",
  freezetime: "freeze time",
  live: "round live",
  over: "round over",
  halftime: "halftime",
  gameover: "match over",
};

function statusLine(live) {
  const v = live.view;
  if (!live.connected) {
    return `<span class="dot"></span><span class="label">waiting for cs2</span>`;
  }
  if (v.phase === "menu") {
    return `<span class="dot dim"></span><span class="label">cs2 is open, in the menu</span>`;
  }
  const parts = [`<b>${esc(mapName(v.map))}</b>`];
  if (v.score) parts.push(`<span class="num">${v.score[0]}:${v.score[1]}</span>`);
  let phase = PHASE[v.phase] ?? v.phase;
  if (v.phase === "live" && v.alive === false) phase = "spectating";
  parts.push(phase);
  const counted = live.ranked ? "" : `<span class="pill quiet" data-tip="only premier and competitive count">${esc(modeName(v.mode))}</span>`;
  return `<span class="dot on"></span><span class="label">${parts.join(`<span class="sep"> · </span>`)}</span><span class="grow"></span>${counted}`;
}

function musicPill(live) {
  if (!S.prefs.music) return `<span class="pill quiet" data-tip="turn it on in settings"><i></i>music control off</span>`;
  if (live.musicPaused) return `<span class="pill" data-tip="resumes when you die or the round ends"><i></i>music paused</span>`;
  return "";
}

function setupCard() {
  const st = S.setup;
  if (st.listenError) {
    return `<div class="card alert">
      <h3>Can't listen for CS2</h3>
      <p>${esc(st.listenError)} freezetime can move to a free one and update CS2's file to match.</p>
      <div class="actions"><button class="btn primary small" data-act="change_port">use another port</button></div>
    </div>`;
  }
  if (!st.status) {
    return `<div class="card alert">
      <h3>CS2 not found</h3>
      <p>freezetime looks for CS2 through Steam's library list and didn't find it. If it's installed, point to its folder: the one called <code>Counter-Strike Global Offensive</code>.</p>
      <div class="actions"><button class="btn primary small" data-act="pick">find cs2 folder</button></div>
    </div>`;
  }
  if (st.status !== "installed") {
    return `<div class="card alert">
      <h3>Connect CS2</h3>
      <p>CS2 sends match updates to apps that ask through a small file in its cfg folder. This is the only thing freezetime puts in the game's folder.</p>
      <p class="small"><code>${esc(st.cfgPath)}</code></p>
      <div class="actions"><button class="btn primary small" data-act="install">add the file</button></div>
    </div>`;
  }
  return "";
}

function staleCard(live) {
  if (!live.staleToken) return "";
  return `<div class="card alert">
    <h3>Restart CS2</h3>
    <p>CS2 is still using an older version of freezetime's file. It reads the file when it starts, so close the game and open it again.</p>
  </div>`;
}

function justInstalledCard(live) {
  if (live.everConnected || live.staleToken || S.setup.status !== "installed" || S.setup.listenError) return "";
  return `<div class="card">
    <h3>Ready</h3>
    <p>Start CS2, or restart it if it's already open: it reads the file at launch. The dot above turns white when the first update arrives.</p>
  </div>`;
}

function messageCards(live) {
  return live.messages
    .filter((m) => m.kind !== "recap")
    .map(
      (m) => `<div class="card${m.kind === "tilt" ? " alert" : ""}">
        <div class="meta">${m.kind === "tilt" ? "tilt guard" : "break"} · ${clock(m.at)}</div>
        <h3>${esc(m.title)}</h3>
        <p>${esc(m.body)}</p>
        <button class="x" data-dismiss="${m.id}" aria-label="Dismiss" data-tip="dismiss">${ICON.x}</button>
      </div>`
    )
    .join("");
}

function renderSession() {
  const live = S.live;
  if (live.recap && !live.session) return renderRecap(live.recap, { live: true });
  const sess = live.session;
  const aside = sess ? `<span class="aside" data-tip="started ${clock(sess.started)}">${dur(now() - sess.started)}</span>` : "";
  let html = `<div class="page">
    <div class="title-row"><h1>session</h1>${aside}</div>
    <div class="status">${statusLine(live)}${musicPill(live)}</div>
    ${setupCard()}${staleCard(live)}${justInstalledCard(live)}${messageCards(live)}`;

  if (!sess || sess.tally.matches === 0) {
    if (S.setup.status !== "installed" || S.setup.listenError) {
      main.innerHTML = html + "</div>";
      return;
    }
    const inMatch = live.connected && live.view.phase !== "menu";
    html += `<div class="empty">
      <div class="big">${inMatch ? "first match underway" : "no matches yet"}</div>
      <p>${
        inMatch
          ? "Results show up here when the match ends."
          : "A session starts with your next match and ends when you close CS2. Premier and Competitive results count toward your numbers."
      }</p>
    </div></div>`;
    main.innerHTML = html;
    return;
  }

  const t = sess.tally;
  const streak = sess.streak > 0 ? `W${sess.streak}` : sess.streak < 0 ? `L${-sess.streak}` : "0";
  html += `<div class="tiles">
      ${tile("matches", t.matches)}
      ${tile("record", record(t))}
      ${tile("k/d", sess.kd)}
      ${tile("streak", streak, sess.streak === 0 ? " dim" : "")}
    </div>
    <div class="group"><span>matches</span><span>${t.kills} kills · ${t.deaths} deaths</span></div>
    <div class="rows">${sess.matches.map((m) => matchRow(m)).join("")}</div>
    <div class="page-end"><button class="btn ghost small" data-act="end">end session now</button></div>
  </div>`;
  main.innerHTML = html;
}

/* ---------- recap ---------- */

function renderRecap(r, { live = false } = {}) {
  const t = r.tally;
  const rate = t.matches ? Math.round((t.wins * 100) / t.matches) : 0;
  const top = live
    ? `<div class="title-row"><h1>recap</h1><span class="aside">${day(r.started)}</span></div>`
    : `<button class="back" data-act="back">${ICON.back}history</button>
       <div class="title-row"><h1>recap</h1><span class="aside">${day(r.started)}</span></div>`;
  const best = r.maps.find((m) => m.map === r.best);
  const worst = r.maps.find((m) => m.map === r.worst);
  const cell = (k, l) =>
    l ? `<div><div class="k">${k}</div><div class="v">${esc(mapName(l.map))}</div><div class="s">${record(l.tally)} · ${l.kd} k/d</div></div>` : "";
  const pair = best || worst ? `<div class="pair">${cell("best map", best)}${cell("worst map", worst)}</div>` : "";
  main.innerHTML = `<div class="page">
    ${top}
    <div class="status"><span class="label">${clock(r.started)} to ${clock(r.ended)} · ${dur(r.ended - r.started)}</span></div>
    <div class="record">
      <div class="big">${record(t)}</div>
      <div class="what">won <b>${t.wins} of ${t.matches}</b><br>${rate}% win rate</div>
    </div>
    <div class="tiles">
      ${tile("k/d", r.kd)}
      ${tile("kills", t.kills)}
      ${tile("deaths", t.deaths)}
      ${tile("worst run", r.longestLossStreak ? `L${r.longestLossStreak}` : "none", r.longestLossStreak ? "" : " dim")}
    </div>
    ${pair}
    ${r.maps.length > 1 ? `<div class="group"><span>maps</span></div>${mapTable(r.maps)}` : ""}
    <div class="group"><span>matches</span><span>${t.matches}</span></div>
    <div class="rows">${r.matches.slice().reverse().map((m) => matchRow(m)).join("")}</div>
    ${live ? `<div class="page-end"><button class="btn primary" data-act="done">done</button></div>` : ""}
  </div>`;
}

/* ---------- history ---------- */

function renderHistory() {
  if (openRecap) return renderRecap(openRecap);
  if (!H) {
    main.innerHTML = `<div class="page"><div class="title-row"><h1>history</h1></div></div>`;
    return;
  }
  const o = H.overall;
  if (o.matches === 0) {
    main.innerHTML = `<div class="page">
      <div class="title-row"><h1>history</h1></div>
      <div class="empty"><div class="big">nothing here yet</div>
      <p>Every Premier and Competitive match you finish is kept here, on this PC. Your tilt numbers come from it, so they get better the more you play.</p></div>
    </div>`;
    return;
  }
  const base = `<tr class="base"><td>any match</td><td>${o.matches}</td><td>${H.winRate ?? "-"}%</td></tr>`;
  const streakRows = base + H.streaks
    .map((s) => {
      const label = s.after === 1 ? "after a loss" : `after ${s.after} losses in a row`;
      const value = s.winRate == null ? `<td class="dim">${s.matches ? "not enough yet" : "none yet"}</td>` : `<td>${s.winRate}%</td>`;
      return `<tr><td>${label}</td><td class="dim">${s.matches}</td>${value}</tr>`;
    })
    .join("");
  const sessions = H.sessions
    .map(
      (r, i) => `<div class="mrow click" data-recap="${i}" role="button" tabindex="0">
        <div class="info">
          <div class="name">${day(r.started)}</div>
          <div class="sub">${clock(r.started)} · ${dur(r.ended - r.started)} · ${r.tally.matches} ${r.tally.matches === 1 ? "match" : "matches"}</div>
        </div>
        <div class="right"><div class="a">${record(r.tally)}</div><div class="b">${r.kd} k/d</div></div>
        ${ICON.chev}
      </div>`
    )
    .join("");
  main.innerHTML = `<div class="page">
    <div class="title-row"><h1>history</h1><span class="aside">${o.matches} matches</span></div>
    <div class="tiles three">
      ${tile("win rate", H.winRate == null ? "-" : `${H.winRate}<small>%</small>`)}
      ${tile("record", record(o))}
      ${tile("k/d", H.kd)}
    </div>
    <div class="group"><span>your numbers</span><span>matches · win rate</span></div>
    <table class="table"><tbody>${streakRows}</tbody></table>
    <div class="group"><span>maps</span></div>
    ${mapTable(H.maps)}
    <div class="group"><span>sessions</span><span>${H.sessions.length}</span></div>
    <div class="rows">${sessions || `<p class="setting d">Finished sessions show up here.</p>`}</div>
  </div>`;
}

/* ---------- settings ---------- */

function sw(key, on, label) {
  return `<button class="switch" role="switch" aria-checked="${on}" aria-label="${esc(label)}" data-pref="${key}"></button>`;
}

function seg(key, options, value) {
  return `<div class="seg" role="radiogroup">${options
    .map(([v, label]) => `<button role="radio" aria-checked="${v === value}" data-seg="${key}" data-value="${v}">${label}</button>`)
    .join("")}</div>`;
}

function renderSettings() {
  const p = S.prefs;
  const st = S.setup;
  let cs2;
  if (!st.status) {
    cs2 = `<div class="setting"><div class="text"><div class="t">CS2 not found</div>
      <div class="d">Point to the folder called Counter-Strike Global Offensive.</div></div>
      <div class="control"><button class="btn small" data-act="pick">find folder</button></div></div>`;
  } else {
    const label = { installed: "Connected", missing: "Not connected", outdated: "Needs updating" }[st.status];
    const button =
      st.status === "installed"
        ? `<button class="btn small" data-act="remove">remove file</button>`
        : `<button class="btn primary small" data-act="install">add file</button>`;
    cs2 = `<div class="setting stack">
      <div class="text"><div class="t">${label}</div>
      <div class="d">CS2 reads freezetime's file when it starts and sends match updates to <code>127.0.0.1:${st.port}</code>, with a token so nothing else can.</div>
      <div class="d path">${esc(st.cfgPath)}</div></div>
      <div class="row-actions">${button}<button class="btn ghost small" data-act="pick">use another folder</button></div>
    </div>`;
  }
  const win = S.platform === "windows";
  main.innerHTML = `<div class="page">
    <div class="title-row"><h1>settings</h1></div>

    <div class="group"><span>cs2</span></div>
    ${cs2}

    <div class="group"><span>music</span></div>
    <div class="setting">
      <div class="text"><div class="t">Pause music during rounds</div>
      <div class="d">Pauses what's playing when a round goes live, and resumes it when you die, when the round ends and between matches. Works with Spotify, browsers and most players.</div></div>
      ${sw("music", p.music, "Pause music during rounds")}
    </div>

    <div class="group"><span>tilt guard</span></div>
    <div class="setting stack">
      <div class="text"><div class="t">Warn after losses in a row</div>
      <div class="d">Shown when the match that makes the streak ends, with your own win rate after streaks like it.</div></div>
      ${seg("tiltAfter", [[0, "off"], [2, "2"], [3, "3"], [4, "4"], [5, "5"]], p.tiltAfter)}
    </div>

    <div class="group"><span>breaks</span></div>
    <div class="setting stack">
      <div class="text"><div class="t">Remind me to take a break after</div>
      <div class="d">Shown when a match ends, never mid-round. Fifteen minutes out of matches counts as a break.</div></div>
      ${seg("breakMinutes", [[0, "off"], [60, "1h"], [90, "90m"], [120, "2h"], [180, "3h"]], p.breakMinutes)}
    </div>

    <div class="group"><span>app</span></div>
    <div class="setting">
      <div class="text"><div class="t">Notification sound</div>
      <div class="d">${win ? "Windows holds back notifications while a game is fullscreen. To let freezetime's through, add it as a priority app in Windows Settings, System, Notifications." : "Plays with warnings and reminders."}</div></div>
      ${sw("sound", p.sound, "Notification sound")}
    </div>
    ${
      win
        ? `<div class="setting">
      <div class="text"><div class="t">Start with Windows</div>
      <div class="d">Starts in the tray, so it's ready before you open CS2.</div></div>
      ${sw("autostart", p.autostart, "Start with Windows")}
    </div>`
        : ""
    }
    <div class="setting">
      <div class="text"><div class="t">Quit freezetime</div>
      <div class="d">Closing the window keeps it running in the tray, where it does its work.</div></div>
      <div class="control"><button class="btn small" data-act="quit">quit</button></div>
    </div>
    <div class="foot-note"><span>freezetime ${esc(S.version)}</span><span>no network access beyond 127.0.0.1</span></div>
  </div>`;
}

/* ---------- render ---------- */

let lastPage = "";
function render() {
  if (!S) return;
  for (const b of document.querySelectorAll(".tabs button")) b.setAttribute("aria-selected", String(b.dataset.view === view));
  // Same page: keep the scroll position. A different page (say the session
  // turning into its recap) starts at the top.
  const page = `${view}:${openRecap?.session ?? ""}:${view === "session" && S.live.recap && !S.live.session ? "recap" : ""}`;
  const keep = page === lastPage ? main.scrollTop : 0;
  lastPage = page;
  if (view === "session") renderSession();
  else if (view === "history") renderHistory();
  else renderSettings();
  main.scrollTop = keep;
}

async function show(next) {
  if (next === view && !openRecap) return;
  view = next;
  openRecap = null;
  main.scrollTop = 0;
  if (view === "history") {
    render();
    await loadHistory();
  }
  render();
}

/* ---------- events ---------- */

$("tabs").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-view]");
  if (b) show(b.dataset.view);
});

main.addEventListener("click", async (e) => {
  const el = e.target.closest("[data-act], [data-dismiss], [data-pref], [data-seg], [data-recap]");
  if (!el) return;
  if (el.dataset.dismiss) {
    await act("dismiss", { id: Number(el.dataset.dismiss) });
    return;
  }
  if (el.dataset.recap) {
    openRecap = H.sessions[Number(el.dataset.recap)];
    main.scrollTop = 0;
    render();
    return;
  }
  if (el.dataset.pref || el.dataset.seg) {
    const key = el.dataset.pref ?? el.dataset.seg;
    const value = el.dataset.pref ? !S.prefs[key] : Number(el.dataset.value);
    const before = S.prefs[key];
    S.prefs[key] = value;
    render();
    const ok = await act("set_prefs", { prefs: S.prefs });
    if (ok === undefined) {
      S.prefs[key] = before;
      render();
    }
    return;
  }
  switch (el.dataset.act) {
    case "install": {
      const setup = await act("install_cfg");
      if (setup) {
        S.setup = setup;
        render();
        toast("file added. restart cs2 if it's open.");
      }
      break;
    }
    case "remove": {
      const setup = await act("remove_cfg");
      if (setup) {
        S.setup = setup;
        render();
        toast("file removed. cs2 stops sending after a restart.");
      }
      break;
    }
    case "pick": {
      const setup = await act("pick_cs2_folder");
      if (setup) {
        S.setup = setup;
        render();
      }
      break;
    }
    case "change_port": {
      const setup = await act("change_port");
      if (setup) {
        S.setup = setup;
        render();
        toast(`now on port ${setup.port}. restart cs2 if it's open.`);
      }
      break;
    }
    case "end":
      await act("end_session");
      break;
    case "done":
      await act("dismiss_recap");
      break;
    case "quit":
      await act("quit_app");
      break;
    case "back":
      openRecap = null;
      render();
      break;
  }
});

main.addEventListener("keydown", (e) => {
  if ((e.key === "Enter" || e.key === " ") && e.target.dataset.recap) {
    e.preventDefault();
    e.target.click();
  }
});

$("btn-min").addEventListener("click", () => appWindow.minimize());
$("btn-close").addEventListener("click", () => appWindow.hide());

listen("live", (e) => {
  if (!S) return;
  const hadRecap = !!S.live.recap;
  S.live = e.payload;
  // A finished session changes the history tab too.
  if (!hadRecap && S.live.recap) H = null;
  if (view === "session" || (view === "history" && !H)) {
    if (view === "history") loadHistory().then(render);
    else render();
  }
});
listen("prefs-changed", refresh);
window.addEventListener("focus", refresh);

// The session clock only needs a redraw once a minute, and only when someone can see it.
setInterval(() => {
  if (!document.hidden && view === "session" && S?.live.session) render();
}, 60000);

/* ---------- tooltips ---------- */

const tip = $("tip");
let tipFor = null;
let tipShown = false;
let tipTimer = 0;
let tipWarmUntil = 0;

function showTip(el) {
  tip.innerHTML = `<span>${esc(el.dataset.tip)}</span>`;
  tip.classList.remove("show");
  const a = el.getBoundingClientRect();
  const t = tip.getBoundingClientRect();
  const m = 6;
  const x = Math.max(m, Math.min(a.left + a.width / 2 - t.width / 2, window.innerWidth - t.width - m));
  const above = a.bottom + 6 + t.height > window.innerHeight - m;
  tip.style.left = `${x}px`;
  tip.style.top = `${above ? a.top - 6 - t.height : a.bottom + 6}px`;
  void tip.offsetWidth;
  tip.classList.add("show");
  tipShown = true;
}

function hideTip() {
  clearTimeout(tipTimer);
  if (tipShown) tipWarmUntil = Date.now() + 500;
  tipShown = false;
  tipFor = null;
  tip.classList.remove("show");
}

document.addEventListener("mouseover", (e) => {
  const el = e.target.closest?.("[data-tip]") ?? null;
  if (el === tipFor) return;
  hideTip();
  if (!el) return;
  tipFor = el;
  tipTimer = setTimeout(() => {
    if (tipFor === el && el.isConnected) showTip(el);
  }, Date.now() < tipWarmUntil ? 0 : 450);
});
document.addEventListener("mouseout", (e) => {
  if (tipFor && !tipFor.contains(e.relatedTarget)) hideTip();
});
for (const ev of ["mousedown", "wheel", "keydown"]) document.addEventListener(ev, hideTip, { capture: true, passive: true });
window.addEventListener("blur", hideTip);

/* ---------- keyboard ---------- */

document.addEventListener("keydown", (e) => {
  const key = e.key;
  const ctrl = e.ctrlKey || e.metaKey;
  // Browser shortcuts that make no sense in an app.
  if (
    key === "F5" || key === "F12" || key === "F3" || key === "F7" ||
    (ctrl && ["p", "u", "g", "j", "h", "s", "o", "n", "t", "w", "r", "f", "+", "-", "=", "0"].includes(key.toLowerCase())) ||
    (ctrl && e.shiftKey && ["i", "j", "c"].includes(key.toLowerCase())) ||
    (e.altKey && (key === "ArrowLeft" || key === "ArrowRight"))
  ) {
    e.preventDefault();
    return;
  }
  if (ctrl || e.altKey) return;
  if (key === "1") show("session");
  else if (key === "2") show("history");
  else if (key === "3") show("settings");
  else if (key === "Escape" && openRecap) {
    openRecap = null;
    render();
  }
});

document.addEventListener("contextmenu", (e) => e.preventDefault());

refresh();
