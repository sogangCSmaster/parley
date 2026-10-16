// Run against a local Vite server; Playwright is optional test tooling, not a runtime dependency.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import assert from 'node:assert/strict';
const browser = await chromium.launch({ headless: true });
const origin = process.env.TEST_ORIGIN ?? 'http://127.0.0.1:5173';
async function setup({ missing = false, delayed = false, mobile = false } = {}) {
    const context = await browser.newContext({ viewport: mobile ? { width: 390, height: 844 } : { width: 1280, height: 900 } });
    const page = await context.newPage();
    await page.addInitScript(({ delayed }) => {
        localStorage.setItem('parley.locale', 'en');
        localStorage.setItem('parley.prefs', JSON.stringify({ name: 'Tester', lang: 'en', partnerLang: 'ko' }));
        window.captures = [];
        window.releaseMedia = [];
        navigator.mediaDevices.getUserMedia = () => new Promise(resolve => {
            const deliver = () => {
                const canvas = document.createElement('canvas');
                const audio = new AudioContext().createMediaStreamDestination();
                const stream = new MediaStream([...canvas.captureStream().getTracks(), ...audio.stream.getTracks()]);
                window.captures.push(stream);
                resolve(stream);
            };
            if (delayed)
                window.releaseMedia.push(deliver);
            else
                deliver();
        });
        window.screens = [];
        navigator.mediaDevices.getDisplayMedia = () => new Promise(resolve => {
            window.releaseDisplay = () => {
                const stream = document.createElement('canvas').captureStream();
                window.screens.push(stream);
                resolve(stream);
            };
        });
        window.signals = [];
        window.sockets = [];
        class Socket {
            static CONNECTING = 0;
            static OPEN = 1;
            static CLOSING = 2;
            static CLOSED = 3;
            readyState = 0;
            constructor(url) { this.url = url; window.sockets.push(this); queueMicrotask(() => { this.readyState = 1; this.onopen?.(); }); }
            send(raw) { window.signals.push(JSON.parse(raw)); }
            close() { this.readyState = 3; this.onclose?.(); }
            receive(msg) { this.onmessage?.({ data: JSON.stringify(msg) }); }
        }
        window.WebSocket = Socket;
    }, { delayed });
    await page.route('**/api/me', r => r.fulfill({ json: { username: null } }));
    await page.route('**/api/rooms/*', r => r.fulfill(missing ? { status: 404 } : { json: { translation: true } }));
    await page.goto(origin + '/abcd-efgh-ijkm');
    return { page, context };
}
async function join(page) { await page.getByRole('button', { name: 'Ask to join', exact: true }).click(); await page.waitForFunction(() => window.signals.some(s => s.t === 'join')); }
async function receive(page, msg) { await page.evaluate(msg => window.sockets.at(-1).receive(msg), msg); }
try {
    for (const delayed of [false, true]) {
        const { page, context } = await setup({ missing: true, delayed });
        await page.getByRole('heading', { name: 'Can’t join', exact: true }).waitFor();
        if (delayed)
            await page.evaluate(() => window.releaseMedia.forEach(fn => fn()));
        await page.waitForFunction(() => window.captures.every(s => s.getTracks().every(t => t.readyState === 'ended')));
        console.log('PASS missing room stops ' + (delayed ? 'late' : 'active') + ' capture');
        await context.close();
    }
    {
        const { page, context } = await setup();
        await join(page);
        assert.equal(await page.getByRole('button', { name: 'Turn off camera', exact: true }).isDisabled(), true);
        assert.equal(await page.getByRole('button', { name: 'Close captions panel', exact: true }).count(), 0);
        await receive(page, { t: 'waiting' });
        await page.getByRole('heading', { name: 'Asking to join…' }).waitFor();
        await page.evaluate(() => window.sockets.at(-1).close());
        await page.getByRole('button', { name: 'Try again', exact: true }).waitFor();
        console.log('PASS waiting disconnect exposes retry');
        await context.close();
    }
    {
        const { page, context } = await setup({ mobile: true });
        await join(page);
        await receive(page, { t: 'welcome', you: 'self', host: true, settings: { translation: true }, peers: [], knocks: [{ id: 'guest', name: 'Guest' }], ice_servers: [] });
        await page.getByRole('button', { name: 'Admit', exact: true }).click();
        await receive(page, { t: 'error', code: 'room_full', message: 'RoomFull' });
        assert.equal(await page.getByRole('button', { name: 'Admit', exact: true }).count(), 1);
        await receive(page, { t: 'knock_resolved', id: 'guest' });
        await page.getByRole('button', { name: 'Admit', exact: true }).waitFor({ state: 'detached' });
        await page.getByRole('button', { name: 'Close captions panel', exact: true }).click();
        const toggle = page.getByRole('button', { name: 'Captions panel', exact: true });
        await toggle.click();
        await page.getByRole('button', { name: 'Close captions panel', exact: true }).waitFor();
        console.log('PASS failed admit retains request and mobile captions can reopen');
        await context.close();
    }
    for (const leaveDuringPicker of [false, true]) {
        const { page, context } = await setup();
        await join(page);
        await receive(page, { t: 'welcome', you: 'self', host: true, settings: { translation: true }, peers: [], knocks: [], ice_servers: [] });
        await page.getByRole('button', { name: 'Present screen', exact: true }).click();
        await page.waitForFunction(() => !!window.releaseDisplay);
        if (leaveDuringPicker) {
            await page.getByRole('button', { name: 'Leave call', exact: true }).click();
        } else {
            await page.getByRole('button', { name: 'Turn off microphone', exact: true }).click();
        }
        await page.evaluate(() => window.releaseDisplay());
        if (!leaveDuringPicker) {
            await page.getByRole('button', { name: 'Stop presenting', exact: true }).waitFor();
            assert.equal(await page.getByRole('button', { name: 'Turn on microphone', exact: true }).count(), 1);
            assert.equal(await page.evaluate(() => window.signals.filter(s => s.t === 'media').at(-1).media.mic), false);
            await page.getByRole('button', { name: 'Leave call', exact: true }).click();
        }
        await page.waitForFunction(() => window.screens.every(s => s.getTracks().every(t => t.readyState === 'ended')));
        console.log('PASS screen share ' + (leaveDuringPicker ? 'stops capture arriving after leave' : 'preserves mute and stops on leave'));
        await context.close();
    }
}
finally {
    await browser.close();
}
