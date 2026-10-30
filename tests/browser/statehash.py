"""A hash of everything the player shows, computed inside the page.

Shared by test_determinism.py and test_smoke.py. Each stage part is hashed in the
page and reported per part; focus, scroll and the merge ghost's slide are left out.
"""

# The function text is re-sent on each evaluate so nothing goes stale across re-runs.
STATE_JS = r"""
() => {
  function cyrb53(str) {
    let h1 = 0xdeadbeef, h2 = 0x41c6ce57;
    for (let i = 0; i < str.length; i++) {
      const ch = str.charCodeAt(i);
      h1 = Math.imul(h1 ^ ch, 2654435761);
      h2 = Math.imul(h2 ^ ch, 1597334677);
    }
    h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507);
    h1 ^= Math.imul(h2 ^ (h2 >>> 13), 3266489909);
    h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507);
    h2 ^= Math.imul(h1 ^ (h1 >>> 13), 3266489909);
    return (4294967296 * (2097151 & h2) + (h1 >>> 0)).toString(36) + ':' + str.length;
  }
  const q = id => document.getElementById(id);

  const view = q('view');
  let viewHtml;
  if (view.querySelector('.absorbing')) {
    // The merge ghost's slide is the one animation-frame mutation; see above.
    const copy = view.cloneNode(true);
    copy.querySelectorAll('.absorbing').forEach(n => {
      const cls = (n.getAttribute('class') || '').split(/\s+/)
        .filter(c => c && c !== 'gone').join(' ');
      n.setAttribute('class', cls);
      n.removeAttribute('style');
    });
    viewHtml = copy.outerHTML;
  } else {
    viewHtml = view.outerHTML;
  }

  const tabs = Array.from(document.querySelectorAll('#tabs button')).map(b =>
    [b.id, b.disabled, b.getAttribute('aria-selected'), b.getAttribute('tabindex'),
     b.getAttribute('title')]);
  const reasons = q('tab-reasons') ? q('tab-reasons').textContent : '';
  const scrub = q('scrub');
  const sel = document.querySelector('#tabs [aria-selected="true"]');
  const parts = {
    counter: scrub.getAttribute('aria-valuetext'),
    tabs: JSON.stringify(tabs) + '|' + reasons,
    view: viewHtml,
    explain: q('explain').outerHTML,
    player: q('controls').outerHTML + '|' +
            JSON.stringify([scrub.value, scrub.max, scrub.disabled]),
    messages: q('messages').outerHTML
  };
  const h = {};
  for (const k of Object.keys(parts)) h[k] = cyrb53(parts[k]);
  const a = document.activeElement;
  return {
    h: h,
    counter: parts.counter,
    tab: sel ? sel.id.replace(/^tab-/, '') : null,
    phase: (q('explain-phase').textContent || '').trim(),
    line: q('explain-summary').textContent,
    active: a ? (a.id || a.localName) : null
  };
}
"""

PARTS = ('counter', 'tabs', 'view', 'explain', 'player', 'messages')


def read(app):
    """The stage's hashes plus a few readable fields; see the module docstring."""
    return app.page.evaluate(STATE_JS)


def differing(got, want, parts=PARTS):
    """The names of the parts whose hashes differ between two reads."""
    return [k for k in parts if got['h'].get(k) != want['h'].get(k)]
