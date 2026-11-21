/* DeepSeek Price Clock: GNOME Shell top-bar indicator.
 *
 * Logic by deepakness (https://github.com/deepakness/deepseek-price-clock,
 * MIT); this file is the GNOME Shell port of his extension.
 * Panel shows the whale and a countdown, green for off-peak (half price) and
 * red for peak. Click for the countdown, 24-hour timeline, upcoming windows
 * and current rates.
 */

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import St from 'gi://St';
import Clutter from 'gi://Clutter';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {
    MODELS,
    SWITCH_MINUTES,
    currentWindow,
    currentWindowStart,
    displaySegments,
    windowList,
} from './price.js';

// A flip older than this when the shell notices it (laptop asleep) is skipped
// rather than announced late.
const NOTIFY_GRACE_MS = 5 * 60000;
const RATES_URL = 'https://deepakness.com/deepseek/';

function pad(n) {
    return String(n).padStart(2, '0');
}

function clock(totalSeconds) {
    const sec = Math.max(0, Math.floor(totalSeconds));
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = sec % 60;
    return `${h}:${pad(m)}:${pad(s)}`;
}

// Panel form: h:mm far out, m:ss inside the last hour.
function shortClock(ms) {
    const sec = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(sec / 3600);
    if (h)
        return `${h}:${pad(Math.floor((sec % 3600) / 60))}`;
    return `${pad(Math.floor(sec / 60))}:${pad(sec % 60)}`;
}

function awayLabel(ms) {
    const mins = Math.max(0, Math.floor(ms / 60000));
    const hours = Math.floor(mins / 60);
    return hours ? `${hours}h ${mins % 60}m` : `${mins}m`;
}

function durationLabel(ms) {
    const mins = Math.max(0, Math.round(ms / 60000));
    const hours = Math.floor(mins / 60);
    const rest = mins % 60;
    if (!hours)
        return `${rest}m`;
    return rest ? `${hours}h ${rest}m` : `${hours}h`;
}

function timeLabel(date, utc) {
    const secs = Math.floor(date.getTime() / 1000);
    const dt = utc
        ? GLib.DateTime.new_from_unix_utc(secs)
        : GLib.DateTime.new_from_unix_local(secs);
    return dt.format('%H:%M');
}

function dayTimeLabel(date, utc) {
    const secs = Math.floor(date.getTime() / 1000);
    const dt = utc
        ? GLib.DateTime.new_from_unix_utc(secs)
        : GLib.DateTime.new_from_unix_local(secs);
    return dt.format('%a %H:%M');
}

function sameDay(a, b, utc) {
    const key = d => {
        const secs = Math.floor(d.getTime() / 1000);
        return (utc ? GLib.DateTime.new_from_unix_utc(secs)
            : GLib.DateTime.new_from_unix_local(secs)).format('%Y-%m-%d');
    };
    return key(a) === key(b);
}

// "Sat 04:00" when the flip crosses midnight, plain time otherwise.
function flipLabel(date, from, utc) {
    return sameDay(date, from, utc) ? timeLabel(date, utc) : dayTimeLabel(date, utc);
}

export default class DeepSeekPriceClockExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._zone = this._settings.get_string('timezone') === 'utc' ? 'utc' : 'local';
        this._lastKind = this._settings.get_string('last-kind');
        this._timeoutId = null;
        this._windowRows = [];

        this._button = new PanelMenu.Button(0.0, 'DeepSeek Price Clock', false);
        this._light = false;
        this._buildPanel();
        this._buildMenu();
        this._applyThemeVariant();
        Main.panel.addToStatusArea(this.uuid, this._button, 0, 'right');

        // Follow the shell between dark and light as the user flips it.
        this._stSettings = St.Settings.get();
        this._schemeSignals = [
            this._stSettings.connect('notify::color-scheme',
                () => this._applyThemeVariant()),
        ];
        // `shell-color-scheme` only exists on newer shells, so probe it.
        try {
            this._schemeSignals.push(this._stSettings.connect(
                'notify::shell-color-scheme', () => this._applyThemeVariant()));
        } catch {
            // Older GNOME: color-scheme is the only signal that matters.
        }

        this._tick();
        this._timeoutId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 1, () => {
            this._tick();
            return GLib.SOURCE_CONTINUE;
        });
    }

    disable() {
        if (this._schemeSignals && this._stSettings) {
            for (const id of this._schemeSignals)
                this._stSettings.disconnect(id);
        }
        this._schemeSignals = null;
        this._stSettings = null;

        if (this._timeoutId) {
            GLib.source_remove(this._timeoutId);
            this._timeoutId = null;
        }
        this._button?.destroy();
        this._button = null;
        this._windowRows = [];
        this._settings = null;
    }

    get _utc() {
        return this._zone === 'utc';
    }

    get _offsetMinutes() {
        return this._utc ? 0 : -new Date().getTimezoneOffset();
    }

    _buildPanel() {
        this._panelLabel = new St.Label({
            style_class: 'dsc-panel-label',
            y_align: Clutter.ActorAlign.CENTER,
            text: '🐳 –:––',
        });
        this._button.add_child(this._panelLabel);
    }

    _buildMenu() {
        this._buildHeader();
        this._buildHero();
        this._buildTimeline();
        this._buildWindows();
        this._buildRates();

        this._button.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        this._notifyItem = new PopupMenu.PopupSwitchMenuItem(
            'Notify me when the price flips',
            this._settings.get_boolean('notify')
        );
        this._notifyItem.connect('toggled', (_item, state) => {
            this._settings.set_boolean('notify', state);
        });
        this._button.menu.addMenuItem(this._notifyItem);

        const link = new PopupMenu.PopupMenuItem('Full price clock ↗');
        link.connect('activate', () => {
            Gio.AppInfo.launch_default_for_uri(RATES_URL, null);
        });
        this._button.menu.addMenuItem(link);
    }

    // A non-reactive menu item with a vertical box we can fill with widgets.
    _section(styleClass) {
        const item = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
            style_class: 'dsc-item',
        });
        const box = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            style_class: styleClass,
        });
        item.add_child(box);
        this._button.menu.addMenuItem(item);
        return box;
    }

    _buildHeader() {
        const box = this._section('dsc-header');
        const row = new St.BoxLayout({x_expand: true, style_class: 'dsc-header-row'});

        const title = new St.Label({
            text: 'DeepSeek Price Clock',
            style_class: 'dsc-title',
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
        });
        row.add_child(title);

        const toggles = new St.BoxLayout({style_class: 'dsc-toggle-group'});
        this._zoneButtons = {};
        for (const [zone, label] of [['local', 'Local'], ['utc', 'UTC']]) {
            const btn = new St.Button({
                label,
                style_class: 'dsc-toggle',
                can_focus: true,
            });
            btn.connect('clicked', () => this._setZone(zone));
            toggles.add_child(btn);
            this._zoneButtons[zone] = btn;
        }
        row.add_child(toggles);
        box.add_child(row);

        this._zoneLabel = new St.Label({style_class: 'dsc-zone'});
        box.add_child(this._zoneLabel);
        this._refreshZone();
    }

    _buildHero() {
        const box = this._section('dsc-hero');

        const badge = new St.BoxLayout({
            style_class: 'dsc-badge',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._dot = new St.Widget({
            style_class: 'dsc-dot',
            y_align: Clutter.ActorAlign.CENTER,
        });
        badge.add_child(this._dot);
        this._badgeLabel = new St.Label({
            style_class: 'dsc-badge-text',
            y_align: Clutter.ActorAlign.CENTER,
        });
        badge.add_child(this._badgeLabel);
        box.add_child(badge);

        this._countLabel = new St.Label({style_class: 'dsc-count'});
        box.add_child(this._countLabel);

        this._untilLabel = new St.Label({style_class: 'dsc-until'});
        box.add_child(this._untilLabel);
    }

    _buildTimeline() {
        const box = this._section('dsc-timeline-box');

        this._area = new St.DrawingArea({
            style_class: 'dsc-track',
            x_expand: true,
        });
        this._area.connect('repaint', () => this._drawTimeline());
        box.add_child(this._area);

        const ticks = new St.BoxLayout({style_class: 'dsc-ticks', x_expand: true});
        const marks = ['00', '06', '12', '18', '24'];
        marks.forEach((mark, i) => {
            const label = new St.Label({
                text: mark,
                style_class: 'dsc-tick',
                x_expand: true,
            });
            if (i === 0)
                label.x_align = Clutter.ActorAlign.START;
            else if (i === marks.length - 1)
                label.x_align = Clutter.ActorAlign.END;
            else
                label.x_align = Clutter.ActorAlign.CENTER;
            ticks.add_child(label);
        });
        box.add_child(ticks);
    }

    _buildWindows() {
        const box = this._section('dsc-windows');
        this._windowsCaption = new St.Label({style_class: 'dsc-caption'});
        box.add_child(this._windowsCaption);

        for (let i = 0; i < SWITCH_MINUTES.length; i++) {
            const row = new St.BoxLayout({x_expand: true, style_class: 'dsc-window-row'});
            const kind = new St.Label({text: '…', style_class: 'dsc-win-kind'});
            const range = new St.Label({text: '…', style_class: 'dsc-win-range', x_expand: true});
            const meta = new St.Label({text: '', style_class: 'dsc-win-meta'});
            row.add_child(kind);
            row.add_child(range);
            row.add_child(meta);
            box.add_child(row);
            this._windowRows.push({row, kind, range, meta});
        }
    }

    _buildRates() {
        const box = this._section('dsc-rates');

        const head = new St.BoxLayout({x_expand: true, style_class: 'dsc-rates-head'});
        this._ratesLabel = new St.Label({style_class: 'dsc-caption', x_expand: true});
        head.add_child(this._ratesLabel);
        head.add_child(new St.Label({text: 'per 1M tokens', style_class: 'dsc-caption'}));
        box.add_child(head);

        this._rateAmounts = [];
        for (const model of MODELS) {
            const row = new St.BoxLayout({x_expand: true, style_class: 'dsc-rate-row'});
            row.add_child(new St.Label({text: model.name, style_class: 'dsc-rate-name', x_expand: true}));
            const amounts = [];
            for (let i = 0; i < 3; i++) {
                const amt = new St.Label({text: '…', style_class: 'dsc-amt'});
                row.add_child(amt);
                amounts.push(amt);
            }
            box.add_child(row);
            this._rateAmounts.push({model, amounts});
        }

        this._nextRatesLabel = new St.Label({style_class: 'dsc-next-rates'});
        box.add_child(this._nextRatesLabel);
    }

    _refreshZone() {
        this._zoneLabel.text = this._utc
            ? 'UTC · all windows in UTC'
            : `Local time · GMT${this._offsetMinutes >= 0 ? '+' : '−'}${this._offsetString()}`;

        for (const [zone, btn] of Object.entries(this._zoneButtons)) {
            const active = zone === this._zone;
            btn.remove_style_class_name('dsc-toggle-active');
            if (active)
                btn.add_style_class_name('dsc-toggle-active');
        }
    }

    _offsetString() {
        const abs = Math.abs(this._offsetMinutes);
        const hours = Math.floor(abs / 60);
        const minutes = abs % 60;
        return `${hours}${minutes ? `:${pad(minutes)}` : ''}`;
    }

    // Text and panel colors in stylesheet.css are tuned for the dark shell; the
    // `dsc-light` class swaps in the light palette when the shell is light.
    _applyThemeVariant() {
        if (!this._button)
            return;
        this._light = Main.getStyleVariant() === 'light';
        const actors = [this._button, this._button.menu?.actor];
        for (const actor of actors.filter(a => a)) {
            actor.remove_style_class_name('dsc-light');
            if (this._light)
                actor.add_style_class_name('dsc-light');
        }
        this._area?.queue_repaint();
    }

    _setZone(zone) {
        if (zone === this._zone)
            return;
        this._zone = zone;
        this._settings.set_string('timezone', zone);
        this._refreshZone();
        this._area.queue_repaint();
        this._tick();
    }

    _tick() {
        if (!this._button)
            return;

        const now = new Date();
        const win = currentWindow(now);
        const peak = win.kind === 'peak';
        const remaining = win.end.getTime() - now.getTime();

        this._updateNotifications(now, win);

        // Panel.
        this._panelLabel.text = `🐳 ${shortClock(remaining)}`;
        this._panelLabel.remove_style_class_name('dsc-panel-peak');
        this._panelLabel.remove_style_class_name('dsc-panel-off');
        this._panelLabel.add_style_class_name(peak ? 'dsc-panel-peak' : 'dsc-panel-off');

        // Hero.
        this._dot.remove_style_class_name('dsc-dot-peak');
        this._dot.remove_style_class_name('dsc-dot-off');
        this._dot.add_style_class_name(peak ? 'dsc-dot-peak' : 'dsc-dot-off');
        this._badgeLabel.text = peak ? 'Peak pricing' : 'Off-peak · half price';
        this._countLabel.text = `${clock(remaining / 1000)} left`;
        this._untilLabel.text = `${peak ? 'Off-peak' : 'Peak'} starts ${flipLabel(win.end, now, this._utc)}`;

        // Timeline.
        this._area.queue_repaint();

        // Windows.
        this._windowsCaption.text = `Price windows · ${this._utc ? 'UTC' : 'your time'}`;
        const items = windowList(now, this._windowRows.length);
        items.forEach((item, i) => {
            const row = this._windowRows[i];
            row.row.remove_style_class_name('dsc-win-peak');
            row.row.remove_style_class_name('dsc-win-off');
            row.row.remove_style_class_name('dsc-win-current');
            row.row.add_style_class_name(item.kind === 'peak' ? 'dsc-win-peak' : 'dsc-win-off');
            if (item.current)
                row.row.add_style_class_name('dsc-win-current');
            row.kind.text = item.kind === 'peak' ? 'Peak' : 'Off-peak';
            row.range.text = sameDay(item.start, item.end, this._utc)
                ? `${timeLabel(item.start, this._utc)} – ${timeLabel(item.end, this._utc)}`
                : `${dayTimeLabel(item.start, this._utc)} – ${dayTimeLabel(item.end, this._utc)}`;
            row.meta.text = item.current
                ? `${awayLabel(item.end.getTime() - now.getTime())} left`
                : durationLabel(item.end.getTime() - item.start.getTime());
        });

        // Rates.
        this._ratesLabel.text = `Paying now · ${peak ? 'peak' : 'off-peak'}`;
        const current = peak ? 'peak' : 'off';
        const next = peak ? 'off' : 'peak';
        for (const {model, amounts} of this._rateAmounts) {
            amounts.forEach((label, i) => {
                label.text = model[current][i];
            });
        }
        const nextOf = MODELS.map(m => `${m.name} ${m[next].join(' / ')}`).join(' · ');
        this._nextRatesLabel.text =
            `From ${flipLabel(win.end, now, this._utc)} · ${next === 'peak' ? 'peak' : 'off-peak'}: ${nextOf}`;
    }

    _updateNotifications(now, win) {
        if (win.kind === this._lastKind)
            return;

        const previous = this._lastKind;
        this._lastKind = win.kind;
        this._settings.set_string('last-kind', win.kind);

        // First run after install only records the kind.
        if (!previous || !this._settings.get_boolean('notify'))
            return;
        if (now.getTime() - currentWindowStart(now).getTime() > NOTIFY_GRACE_MS)
            return;

        const peak = win.kind === 'peak';
        const time = flipLabel(win.end, now, this._utc);
        Main.notify(
            peak ? 'DeepSeek: peak pricing started' : 'DeepSeek: off-peak started (half price)',
            peak ? `Off-peak again ${time}` : `Half price until ${time}`
        );
    }

    _drawTimeline() {
        const cr = this._area.get_context();
        const [width, height] = this._area.get_surface_size();
        if (width <= 0 || height <= 0) {
            cr.$dispose();
            return;
        }

        const now = new Date();
        const segments = displaySegments(now, this._offsetMinutes);
        for (const seg of segments) {
            const x = (seg.start / 1440) * width;
            const w = Math.max(1, ((seg.end - seg.start) / 1440) * width);
            if (seg.kind === 'peak') {
                if (this._light)
                    cr.setSourceRGBA(0.77, 0.11, 0.16, 0.95);
                else
                    cr.setSourceRGBA(0.94, 0.36, 0.33, 0.95);
            } else if (this._light) {
                cr.setSourceRGBA(0.07, 0.50, 0.24, 0.95);
            } else {
                cr.setSourceRGBA(0.34, 0.85, 0.55, 0.95);
            }
            cr.rectangle(x, 1, w, Math.max(1, height - 2));
            cr.fill();
        }

        const minute = (now.getUTCHours() * 60 + now.getUTCMinutes() +
            now.getUTCSeconds() / 60 + this._offsetMinutes + 1440) % 1440;
        if (this._light)
            cr.setSourceRGBA(0.1, 0.1, 0.1, 0.9);
        else
            cr.setSourceRGBA(1, 1, 1, 0.95);
        cr.rectangle((minute / 1440) * width - 1, 0, 2, height);
        cr.fill();
        cr.$dispose();
    }
}
