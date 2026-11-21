/* Tests for price.js. Run with `npm test` (node --test tests/).
 *
 * These check the ported rules against the upstream behaviour: fixed UTC
 * windows, the Aug 2026 weekday-only rule, the 2026/2027 Chinese holidays, and
 * the invariants the UI depends on (a countdown that always ends on a real
 * flip, contiguous price windows, a timeline that covers 24 hours).
 */

import test from 'node:test';
import assert from 'node:assert/strict';

import {
    MODELS,
    SWITCH_MINUTES,
    currentWindow,
    currentWindowStart,
    displaySegments,
    kindAtInstant,
    nextSwitch,
    nextSwitches,
    windowList,
} from '../price.js';

const at = iso => new Date(iso);
const iso = date => date.toISOString().replace('.000Z', 'Z');

// A plain Wednesday, no holiday, no weekend.
const WEDNESDAY = '2026-11-25';

test('weekday switch boundaries are exact to the millisecond', () => {
    const cases = [
        [`${WEDNESDAY}T00:59:59.999Z`, 'off'],
        [`${WEDNESDAY}T01:00:00.000Z`, 'peak'],
        [`${WEDNESDAY}T03:59:59.999Z`, 'peak'],
        [`${WEDNESDAY}T04:00:00.000Z`, 'off'],
        [`${WEDNESDAY}T05:59:59.999Z`, 'off'],
        [`${WEDNESDAY}T06:00:00.000Z`, 'peak'],
        [`${WEDNESDAY}T09:59:59.999Z`, 'peak'],
        [`${WEDNESDAY}T10:00:00.000Z`, 'off'],
        [`${WEDNESDAY}T23:59:59.999Z`, 'off'],
    ];
    for (const [instant, expected] of cases) {
        assert.equal(kindAtInstant(at(instant)), expected, instant);
    }
});

test('the whole weekend is off-peak once the weekday rule applies', () => {
    // Sat 2026-11-28 and Sun 2026-11-29, at times that are peak on a weekday.
    for (const instant of [
        '2026-11-28T01:30:00Z',
        '2026-11-28T02:00:00Z',
        '2026-11-28T07:00:00Z',
        '2026-11-29T02:00:00Z',
        '2026-11-29T09:59:00Z',
    ])
        assert.equal(kindAtInstant(at(instant)), 'off', instant);

    assert.equal(kindAtInstant(at('2026-11-30T02:00:00Z')), 'peak', 'Monday 10:00 Beijing');
});

test('weekend rule starts at 00:00 Beijing on Sunday 2026-08-23', () => {
    // Before the rule, a Saturday followed the normal UTC windows.
    assert.equal(kindAtInstant(at('2026-08-15T02:00:00Z')), 'peak', 'Sat 2026-08-15 10:00 Beijing');
    assert.equal(kindAtInstant(at('2026-08-22T02:00:00Z')), 'peak', 'Sat 2026-08-22 10:00 Beijing');
    // Exactly at the rule start (Sun 00:00 Beijing) and after it.
    assert.equal(kindAtInstant(at('2026-08-22T16:00:00Z')), 'off', 'rule start instant');
    assert.equal(kindAtInstant(at('2026-08-23T02:00:00Z')), 'off', 'Sun 10:00 Beijing');
    assert.equal(kindAtInstant(at('2026-08-24T02:00:00Z')), 'peak', 'Mon 10:00 Beijing');
});

test('Chinese public holidays are off-peak for their whole Beijing day', () => {
    const peakTimesOnHolidays = [
        ['2027-01-01T02:00:00Z', 'New Year, Fri 10:00 Beijing'],
        ['2027-02-05T02:00:00Z', 'Spring Festival, first day'],
        ['2027-02-08T02:00:00Z', 'Spring Festival, Monday inside the break'],
        ['2027-02-13T03:00:00Z', 'Spring Festival, last day'],
        ['2027-02-14T03:00:00Z', 'Sunday after Spring Festival'],
        ['2027-04-05T03:00:00Z', 'Qingming, Monday'],
        ['2027-05-03T02:00:00Z', 'Labour Day, Monday'],
        ['2027-06-09T02:00:00Z', 'Dragon Boat, single day'],
        ['2027-09-15T02:00:00Z', 'Mid-Autumn, single day'],
        ['2027-10-04T02:00:00Z', 'National Day, Monday'],
        ['2027-10-07T02:00:00Z', 'National Day, last day'],
    ];
    for (const [instant, label] of peakTimesOnHolidays)
        assert.equal(kindAtInstant(at(instant)), 'off', label);

    // The days on either side are normal weekdays.
    assert.equal(kindAtInstant(at('2027-02-04T02:00:00Z')), 'peak', 'day before Spring Festival');
    assert.equal(kindAtInstant(at('2027-02-15T02:00:00Z')), 'peak', 'Monday after Spring Festival');
    assert.equal(kindAtInstant(at('2027-06-10T02:00:00Z')), 'peak', 'day after Dragon Boat');
    assert.equal(kindAtInstant(at('2027-10-08T02:00:00Z')), 'peak', 'day after National Day');
});

test('nextSwitch always lands on a real flip, strictly in the future', () => {
    const samples = [
        '2026-11-25T00:00:00Z',
        '2026-11-25T00:59:59.999Z',
        '2026-11-25T03:59:59.999Z',
        '2026-11-25T09:59:00Z',
        '2026-11-27T12:00:00Z', // Friday afternoon, before the weekend
        '2026-11-28T12:00:00Z', // Saturday
        '2027-02-08T12:00:00Z', // inside Spring Festival
    ];
    for (const instant of samples) {
        const now = at(instant);
        const next = nextSwitch(now);
        assert.ok(next.date.getTime() > now.getTime(), `${instant}: switch is in the future`);
        assert.equal(kindAtInstant(next.date), next.kind, `${instant}: reported kind`);
        assert.notEqual(kindAtInstant(next.date), kindAtInstant(now), `${instant}: kind flips`);
        assert.equal(kindAtInstant(new Date(next.date.getTime() - 1)), kindAtInstant(now), `${instant}: flips exactly at the switch`);
        assert.equal(currentWindow(now).end.getTime(), next.date.getTime(), `${instant}: currentWindow ends on it`);
    }
});

test('an instant that is already a switch skips to the following one', () => {
    const switchInstant = at(`${WEDNESDAY}T01:00:00Z`);
    assert.equal(iso(nextSwitch(switchInstant).date), '2026-11-25T04:00:00Z');
    // One millisecond earlier the same switch is still ahead.
    assert.equal(iso(nextSwitch(new Date(switchInstant.getTime() - 1)).date), '2026-11-25T01:00:00Z');
});

test('nextSwitches returns the requested count, ordered and gated consistently', () => {
    const now = at('2027-02-08T12:00:00Z');
    const list = nextSwitches(now, 4);
    assert.equal(list.length, 4);
    let previous = now.getTime();
    for (const item of list) {
        assert.ok(item.date.getTime() > previous, 'strictly increasing');
        assert.equal(kindAtInstant(item.date), item.kind);
        assert.equal(kindAtInstant(new Date(item.date.getTime() - 1)), item.kind === 'peak' ? 'off' : 'peak');
        previous = item.date.getTime();
    }
    // Nothing lands on a gated weekend/holiday switch.
    assert.equal(iso(list[0].date), '2027-02-15T01:00:00Z');
});

test('currentWindowStart points at the start of the active window', () => {
    const samples = [
        '2026-11-25T00:00:00Z',
        '2026-11-25T01:00:00Z',
        '2026-11-25T03:59:59.999Z',
        '2026-11-28T12:00:00Z',
        '2026-11-29T23:00:00Z',
        '2027-02-08T12:00:00Z',
        '2027-02-15T01:00:00Z',
    ];
    for (const instant of samples) {
        const now = at(instant);
        const start = currentWindowStart(now);
        assert.ok(start.getTime() <= now.getTime(), `${instant}: start is not in the future`);
        assert.equal(kindAtInstant(start), kindAtInstant(now), `${instant}: same kind`);
        assert.equal(iso(nextSwitch(start).date), iso(currentWindow(now).end), `${instant}: next switch from the start is this window's end`);
        assert.ok(nextSwitch(start).date.getTime() > now.getTime(), `${instant}: no switch between the start and now`);
    }

    // Exactly on a switch the window starts right there.
    assert.equal(iso(currentWindowStart(at(`${WEDNESDAY}T06:00:00Z`))), '2026-11-25T06:00:00Z');
    // A long holiday stretch is one window.
    assert.equal(iso(currentWindowStart(at('2027-02-08T12:00:00Z'))), '2027-02-04T10:00:00Z');
});

test('windowList is contiguous and alternates kinds', () => {
    const now = at('2026-11-25T12:34:00Z');
    const list = windowList(now, 4);
    assert.equal(list.length, 4);
    assert.equal(list[0].current, true);
    assert.equal(iso(list[0].start), iso(currentWindowStart(now)));
    assert.equal(list.filter(w => w.current).length, 1);

    for (let i = 0; i < list.length; i++) {
        const window = list[i];
        assert.equal(window.current, i === 0);
        assert.ok(window.end.getTime() > window.start.getTime(), `window ${i} has a positive duration`);
        assert.equal(kindAtInstant(window.start), window.kind, `window ${i} kind`);
        if (i > 0) {
            assert.equal(iso(window.start), iso(list[i - 1].end), `window ${i} starts where ${i - 1} ends`);
            assert.notEqual(window.kind, list[i - 1].kind, `window ${i} flips`);
        }
    }
});

test('windowList handles a holiday week as one off-peak window', () => {
    const now = at('2027-02-08T12:00:00Z');
    const list = windowList(now, 4);
    assert.equal(list[0].kind, 'off');
    assert.equal(iso(list[0].start), '2027-02-04T10:00:00Z');
    assert.equal(iso(list[0].end), '2027-02-15T01:00:00Z');
    assert.equal(list[1].kind, 'peak');
    assert.equal(iso(list[1].start), '2027-02-15T01:00:00Z');
});

test('the 24 hour bar covers the day for every time zone offset', () => {
    const offsets = [0, 330, -180, 840, -720, -60, 1380, 60];
    const nows = [
        '2026-11-25T12:34:00Z',
        '2026-11-25T23:30:00Z',
        '2026-11-26T00:30:00Z',
        '2027-02-08T12:00:00Z',
        '2027-06-09T02:00:00Z',
    ];
    for (const instant of nows) {
        for (const offset of offsets) {
            const now = at(instant);
            const segments = displaySegments(now, offset);
            assert.ok(segments.length > 0, `${instant}/${offset}: has segments`);
            assert.equal(segments[0].start, 0, `${instant}/${offset}: starts at midnight`);
            assert.equal(segments[segments.length - 1].end, 1440, `${instant}/${offset}: ends at midnight`);
            for (let i = 0; i < segments.length; i++) {
                const segment = segments[i];
                assert.ok(segment.end > segment.start, `${instant}/${offset}: segment ${i} is non-empty`);
                if (i > 0) {
                    assert.equal(segment.start, segments[i - 1].end, `${instant}/${offset}: segment ${i} is contiguous`);
                    assert.notEqual(segment.kind, segments[i - 1].kind, `${instant}/${offset}: segment ${i} merges nothing`);
                }
            }
        }
    }
});

test('the bar agrees with the panel state under the now marker', () => {
    const offsets = [0, 330, -180, 840, -720];
    const nows = [
        '2026-11-25T00:10:00Z',
        '2026-11-25T02:00:00Z',
        '2026-11-25T05:00:00Z',
        '2026-11-25T09:00:00Z',
        '2026-11-27T15:00:00Z',
        '2026-11-28T02:00:00Z',
        '2027-02-08T12:00:00Z',
    ];
    for (const instant of nows) {
        for (const offset of offsets) {
            const now = at(instant);
            // Local midnight as an instant, derived from the offset alone.
            const shifted = new Date(now.getTime() + offset * 60000);
            const dayStart = Date.UTC(
                shifted.getUTCFullYear(),
                shifted.getUTCMonth(),
                shifted.getUTCDate()
            ) - offset * 60000;
            const minute = (now.getTime() - dayStart) / 60000;
            const segments = displaySegments(now, offset);
            const segment = segments.find(s => minute >= s.start && minute < s.end);
            assert.ok(segment, `${instant}/${offset}: now falls inside the bar`);
            // Sample inside the segment, away from its edges, and require the
            // drawn kind to match the rules at that instant.
            const sample = new Date(dayStart + (segment.start + segment.end) / 2 * 60000);
            assert.equal(segment.kind, kindAtInstant(sample), `${instant}/${offset}: segment kind`);
        }
    }
});

test('a whole year of hourly instants never reports a bogus switch', () => {
    const start = Date.UTC(2027, 0, 1);
    for (let hour = 0; hour < 365 * 24; hour++) {
        const now = new Date(start + hour * 3600000);
        const next = nextSwitch(now);
        assert.ok(next.date.getTime() > now.getTime(), `next switch after ${now.toISOString()}`);
        assert.notEqual(kindAtInstant(next.date), kindAtInstant(now), `flip at ${now.toISOString()}`);
        assert.equal(next.kind, kindAtInstant(next.date), `kind at ${now.toISOString()}`);
    }
});

test('the switch minutes are sorted and inside a day', () => {
    assert.deepEqual([...SWITCH_MINUTES].sort((a, b) => a - b), SWITCH_MINUTES);
    assert.equal(SWITCH_MINUTES.length % 2, 0, 'windows open and close in pairs');
    for (const minute of SWITCH_MINUTES)
        assert.ok(minute > 0 && minute < 1440, `${minute} is a valid minute of day`);
});

test('peak rates are higher than off-peak for every model', () => {
    assert.ok(MODELS.length > 0);
    for (const model of MODELS) {
        assert.equal(model.off.length, 3, `${model.name} off-peak columns`);
        assert.equal(model.peak.length, 3, `${model.name} peak columns`);
        for (let i = 0; i < 3; i++) {
            const off = Number.parseFloat(model.off[i].replace('$', ''));
            const peak = Number.parseFloat(model.peak[i].replace('$', ''));
            assert.ok(Number.isFinite(off) && Number.isFinite(peak), `${model.name} column ${i} is numeric`);
            assert.ok(peak > off, `${model.name} column ${i}: peak ${peak} > off ${off}`);
        }
    }
});
