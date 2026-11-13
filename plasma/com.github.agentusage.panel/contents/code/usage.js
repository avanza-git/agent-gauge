// Pure formatting helpers, also exercised by tests/panel-widget.cjs.
function quoteShell(value) {
    return "'" + value.replace(/'/g, "'\\''") + "'";
}

function command(executable) {
    return (executable ? quoteShell(executable) : '"$HOME/.local/bin/agent-gauge"') + " --panel-snapshot";
}

function validSnapshot(value) {
    return value && value.version === 1 && typeof value.updated_at === "number"
        && value.updated_at > 0 && typeof value.stale_after === "number" && value.stale_after > 0
        && Array.isArray(value.providers) && value.providers.length === 2
        && value.providers[0].id === "claude" && value.providers[1].id === "codex";
}

function stale(snapshot, now) {
    return !snapshot || now - snapshot.updated_at > snapshot.stale_after;
}

function percent(window) {
    return window && typeof window.utilization === "number" && isFinite(window.utilization)
        ? Math.floor(window.utilization * 100) + "%" : "--";
}

function duration(seconds) {
    var minutes = Math.ceil(seconds / 60);
    if (minutes < 60) return minutes + "m";
    var hours = Math.floor(minutes / 60);
    if (hours < 24) return hours + "h " + (minutes % 60) + "m";
    return Math.floor(hours / 24) + "d " + (hours % 24) + "h";
}

function resetText(window, now) {
    if (!window || !window.reset) return "";
    var seconds = window.reset - now;
    if (seconds <= 0) return "Resets soon";
    return "Resets in " + duration(seconds);
}

// True while the window is projected to run out before it resets.
function exhausting(window, now) {
    return !!window && typeof window.exhausts_at === "number" && window.exhausts_at > now;
}

function forecastText(window, now) {
    return exhausting(window, now) ? "Full in " + duration(window.exhausts_at - now) : "";
}

function providers(snapshot, showClaude, showCodex) {
    var rows = snapshot ? snapshot.providers : [
        { id: "claude", label: "Claude", enabled: true, error: "", session: null, weekly: null },
        { id: "codex", label: "Codex", enabled: true, error: "", session: null, weekly: null }
    ];
    return rows.filter(function(p) { return p.id === "claude" ? showClaude : showCodex; });
}

// 0 = normal, 1 = warning, 2 = critical. A window on pace to run out
// before it resets is at least a warning.
function severity(provider, thresholds, now) {
    var values = [provider.session, provider.weekly].filter(function(w) { return !!w; });
    var maximum = values.reduce(function(v, w) { return Math.max(v, w.utilization); }, 0);
    var limits = (thresholds || []).slice().sort(function(a, b) { return a - b; });
    var level = 0;
    if (limits.length > 1 && maximum >= limits[limits.length - 1]) level = 2;
    else if (limits.length && maximum >= limits[0]) level = 1;
    var pace = now !== undefined && values.some(function(w) { return exhausting(w, now); });
    return pace ? Math.max(level, 1) : level;
}
