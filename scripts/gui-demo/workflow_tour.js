// Recorder-only workflow. Real UI keyboard handlers forward Enter to the fake actor.
(() => {
  const full = window.repomonDemoTour.run;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const shown = el => el && el.getClientRects().length > 0;
  const text = el => (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim();
  async function until(check, name) {
    const end = performance.now() + 15000;
    while (performance.now() < end) {
      const found = check(); if (found) return found;
      await pause(100);
    }
    throw new Error(`Missing ${name}: ${document.body.innerText}`);
  }
  async function key(key, code, metaKey = false, target = document.body) {
    target.dispatchEvent(new KeyboardEvent('keydown', {key, code, metaKey, bubbles: true,
      cancelable: true, keyCode: key === 'Enter' ? 13 : 0, which: key === 'Enter' ? 13 : 0}));
    await pause(250);
  }
  async function button(part) {
    const el = await until(() => [...document.querySelectorAll('button')].find(el => shown(el) &&
      (el.getAttribute('aria-label') || text(el)).includes(part)), part);
    el.focus(); el.click(); await pause(250);
  }
  async function palette(query) {
    await key('k', 'KeyK', true);
    const input = await until(() => document.querySelector('input[aria-label="Search commands, repositories, and lanes"]'), 'palette');
    input.focus(); input.value = query; input.dispatchEvent(new Event('input', {bubbles: true}));
    await until(() => [...document.querySelectorAll('[role="option"]')].some(el => shown(el) && text(el).includes(query)), 'matching palette result');
    await key('Enter', 'Enter', false, input);
    await until(() => !shown(input), 'closed palette');
  }
  window.repomonDemoTour.run = async phase => {
    if (!phase.startsWith('workflow-')) return full(phase);
    const send = (event, extra = {}) => window.webkit.messageHandlers.demoProbe.postMessage({event, phase, ...extra});
    try {
      if (phase === 'workflow-start' || phase === 'workflow-resume') {
        await until(() => document.body.innerText.includes('orbit-api') && document.body.innerText.includes('meadow-web'), 'both repositories');
        await button('fix-nav-focus-trap'); await button('Focused layout');
        await until(() => [...document.querySelectorAll('.xterm-helper-textarea')].some(shown), 'attached terminal input');
      } else if (phase === 'workflow-answer') {
        await key('g', 'KeyG', true);
        await until(() => [...document.querySelectorAll('button[aria-current="true"]')].some(el => text(el).includes('feat-rate-limit-headers')), 'keyboard-selected permission lane');
        const input = await until(() => [...document.querySelectorAll('.xterm-helper-textarea')].find(shown), 'active terminal keyboard input');
        input.focus(); await key('Enter', 'Enter', false, input);
        // Python requires the actor's receipt and the daemon's running transition.
      } else if (phase === 'workflow-switch') {
        await palette('fix/nav-focus-trap');
        await until(() => [...document.querySelectorAll('button[aria-current="true"]')].some(el => text(el).includes('fix-nav-focus-trap')), 'palette-selected lane');
      } else if (phase === 'workflow-terminal') {
        await key('t', 'KeyT', true);
      } else if (phase === 'workflow-tui') {
        await until(() => document.body.innerText.includes('orbit-api') && document.body.innerText.includes('meadow-web'), 'same fleet');
      }
      send('tour-complete');
    } catch (error) { send('tour-error', {message: error.message, stack: error.stack}); }
  };
})();
