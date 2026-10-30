"""Page start-up: the dev server under its own CSP, and the two start-up failures.

The dev page must start with no violation under the strict CSP plus 'self'. An
engine that fails to load and a boot that throws must each be reported as what
it is, on the built bundle (temporary copies) and on the dev page (via routing).

Run:  python3 tests/browser/test_loading.py [--browser firefox] [-k name]
"""

import contextlib
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ behind in the tree
sys.path.insert(0, str(Path(__file__).resolve().parent))

import harness  # noqa: E402 - the path tweak above must come first
from harness import (  # noqa: E402
    DIST, LOCAL_SCHEMES, RENDER_FAILURE_TEXT, ROOT, TABS, VARIANTS, expect, main, test)

WEB = ROOT / 'web'
# The one copy of the shipped policy.
STRICT_CSP_FILE = ROOT / 'scripts' / 'dist-csp.txt'

# Everything the development page loads by URL, relative to the server root.
DEV_FILES = ('', 'style.css', 'app.js', 'grammars.js', 'sets.js', 'table.js', 'trace.js',
             'diagram.js', 'compare.js', 'pkg/lr_engine.js', 'pkg/lr_engine_bg.wasm')

ENGINE_FAILURE = 'The analysis engine could not be loaded.'
PAGE_FAILURE = 'Internal rendering error'
PAGE_FAILURE_HINT = 'This is a bug in the visualizer, not a problem with your grammar.'


# ------------------------------------------------------------------- helpers

def open_page(app, url, allowed, offline, intercept=None):
    """Open `url` in a fresh context, watched like open_app; not waited on (some pages must not boot).

    Requests outside `allowed` (URL prefixes) are aborted and logged as blocked.
    `intercept(route, request)` sees every request first; True means it answered."""
    context = app.context.browser.new_context(
        viewport=harness.DEFAULT_VIEWPORT, color_scheme='light',
        service_workers='block', offline=offline)
    log = harness.PageLog()

    def route(route_, request):
        if intercept is not None and intercept(route_, request):
            return
        if request.url.startswith(allowed):
            route_.continue_()
        else:
            log.blocked.append(request.url)
            route_.abort('blockedbyclient')

    context.route('**/*', route)
    context.add_init_script(script=harness.INIT_SCRIPT)
    page = context.new_page()
    other = harness.App(page, context, log, app.browser_name)
    harness.watch(page, log, main=True)
    page.set_default_timeout(harness.DEFAULT_TIMEOUT_MS)
    page.goto(url)
    return other


@contextlib.contextmanager
def dev_server():
    """Run the documented dev server on a free loopback port; yields its base URL."""
    proc = subprocess.Popen(
        [sys.executable, '-u', '-m', 'http.server', '--bind', '127.0.0.1',
         '-d', str(WEB), '0'],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    try:
        banner = proc.stdout.readline()
        found = re.search(r'port (\d+)', banner)
        expect(found, 'python3 -m http.server did not start (exit %r): %r'
               % (proc.poll(), banner))
        yield 'http://127.0.0.1:%s/' % found.group(1)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        proc.stdout.close()


def strict_policy():
    """The shipped policy's `content`, read from scripts/dist-csp.txt."""
    found = re.search(r'content="([^"]*)"', STRICT_CSP_FILE.read_text(encoding='utf-8'))
    expect(found, '%s does not hold a <meta ... content="..."> line' % STRICT_CSP_FILE)
    return found.group(1)


def policy_of(page):
    """The `content` of each CSP <meta> in the page."""
    return page.evaluate(
        """() => Array.from(document.querySelectorAll(
                   'meta[http-equiv="Content-Security-Policy" i]'))
                 .map(m => m.getAttribute('content'))""")


def error_boxes(page):
    """Each error box in #messages as {tag, text, hint}."""
    return page.evaluate(
        """() => Array.from(document.querySelectorAll('#messages .msg.error')).map(e => ({
                 tag: (e.querySelector('.tag') || {}).textContent || '',
                 text: e.textContent,
                 hint: (e.querySelector('.hint') || {}).textContent || '' }))""")


# ------------------------------------------------------------- the dev server

# Both browsers report the deliberate cross-origin attempts at the end of the
# test; nothing before that may log an error.
CSP_CONSOLE = (r'Content Security Policy', r'Content-Security-Policy', r'Refused to',
               r'Fetch API cannot load', r'violates the')


@test()
def dev_server_page_works_under_its_own_csp(app):
    """`python3 -m http.server -d web`: boots, analyzes and draws, with no CSP violation."""
    for name in DEV_FILES[1:]:
        expect((WEB / name).is_file(), 'web/%s is missing - run ./build.sh first' % name)

    with dev_server() as base:
        dev = open_page(app, base, allowed=(base, 'data:', 'blob:'), offline=False)
        try:
            dev.wait_ready()

            # Every part came by URL from the server.
            status = {url[len(base):]: code for url, code in dev.log.responses
                      if url.startswith(base)}
            for name in DEV_FILES:
                expect(status.get(name) == 200,
                       'dev page did not load %r from the server (got %r)'
                       % (name or 'index.html', status.get(name)))

            # Strict policy plus 'self' in three directives, nothing else.
            strict = strict_policy()
            want = (strict.replace("script-src ", "script-src 'self' ")
                    .replace("style-src ", "style-src 'self' ")
                    .replace("connect-src 'none'", "connect-src 'self'"))
            expect(want != strict and want.count("'self'") == 3,
                   'could not derive the dev policy from %r' % strict)
            got = policy_of(dev.page)
            expect(got == [want],
                   'the dev policy is not "strict plus self in script-src, style-src and '
                   'connect-src":\n      got  %r\n      want %r' % (got, [want]))

            # Firefox probes /favicon.ico on its own and the policy refuses it;
            # exactly that URL is set aside, any other violation fails.
            favicon = base + 'favicon.ico'

            def check_clean(when):
                violations = [v for v in dev.csp_violations() if v['blocked'] != favicon]
                expect(not violations, '%s: CSP violations on the dev page: %r' % (when, violations))
                problems = dev.log.problems(allow_console=(re.escape(favicon),),
                                            allow_network=True)
                expect(not problems, '%s: %s' % (when, '; '.join(problems)))
                failed = [f for f in dev.log.failed if f[0] != favicon]
                expect(not failed, '%s: requests failed: %r' % (when, failed))
                outside = [u for u in dev.log.requests + dev.log.blocked + dev.log.websockets
                           if not u.startswith((base, 'data:', 'blob:'))]
                expect(not outside, '%s: the dev page reached outside its server: %r'
                       % (when, outside))

            check_clean('after boot')
            first = dev.outcome()
            expect(first.ok and first.total > 0,
                   'the opening analysis did not render: %r' % (first,))

            # An analysis typed in by hand, under every method, every tab drawn.
            dev.set_grammar('S -> ( S ) | a')
            dev.set_test_string('( ( a ) )')
            expect(dev.analyze().ok, 'analysis failed on the dev page: %s' % dev.messages())
            for variant in VARIANTS:
                out = dev.choose_variant(variant)
                expect(out.ok and out.total > 0, '%s: no analysis on the dev page: %s'
                       % (variant, out.messages))
                dev.last()
                expect(dev.available_tabs() == list(TABS), '%s: enabled tabs are %s'
                       % (variant, dev.available_tabs()))
                for tab in TABS:
                    dev.tab(tab)
                    text = dev.view_text()
                    expect(text.strip(), '%s: tab %s drew nothing' % (variant, tab))
                    expect(RENDER_FAILURE_TEXT not in text,
                           '%s: tab %s could not be drawn' % (variant, tab))
                expect(not dev.errors(), '%s: error box on the dev page: %r'
                       % (variant, dev.errors()))
            check_clean('after analyzing under all four variants')
            probes = [v for v in dev.csp_violations() if v['blocked'] == favicon]
            app.note('dev server %s: %d files by URL, policy = strict + self x3, '
                     '0 violations by the page, %d requests%s'
                     % (base, len(DEV_FILES), len(dev.log.requests),
                        "; the browser's own /favicon.ico probe was refused (img-src)"
                        if probes else ''))

            # Another origin is still refused by the policy before the request leaves.
            results = dev.page.evaluate(
                """async () => {
                     const out = {};
                     const settle = p => Promise.race(
                       [p, new Promise(r => setTimeout(() => r('pending'), 1500))]);
                     out.fetch = await settle(fetch('https://example.invalid/fetch')
                       .then(() => 'loaded', e => 'blocked: ' + e.name));
                     out.image = await settle(new Promise(r => {
                       const i = new Image(); i.onload = () => r('loaded');
                       i.onerror = () => r('blocked: error event');
                       i.src = 'https://example.invalid/img.png'; }));
                     await new Promise(r => setTimeout(r, 300));
                     return out;
                   }""")
            for api, result in results.items():
                expect(result.startswith('blocked'),
                       'dev policy let %s reach another origin: %s' % (api, result))
            expect(not dev.log.blocked,
                   'requests got past the dev policy to the network layer: %r' % dev.log.blocked)
            directives = sorted({v['directive'].split(' ')[0] for v in dev.csp_violations()})
            expect('connect-src' in directives and 'img-src' in directives,
                   'expected connect-src and img-src violations, got %r' % directives)
            unexpected = [t for t in dev.log.console_errors
                          if not any(re.search(p, t) for p in CSP_CONSOLE)]
            expect(not unexpected, 'console errors beyond the CSP reports: %r' % unexpected)
            expect(not dev.log.page_errors, 'page errors: %r' % dev.log.page_errors)
            app.note('other origins still refused by: %s' % ', '.join(directives))
        finally:
            dev.close()


# ------------------------------------------------- the two start-up failures

# A valid module starts with "\0asm" ("AGFzbQ" in base64); "AAAAAA" decodes to
# zeros there and the compiler refuses it, as for a corrupted engine.
WASM_PREFIX = 'const B64 = "AGFzbQ'
BROKEN_WASM_PREFIX = 'const B64 = "AAAAAA'
LOADER_TAG = '<script type="module">'
BOOT_THROWS = ("<script>window.LRV.boot = function () "
               "{ throw new Error('boot sabotaged by the test'); };</script>\n")
PAGE_SCRIPTS_GONE = '<script>window.LRV = undefined;</script>\n'


def break_engine(text):
    """The bundle with an engine that decodes but does not compile."""
    expect(text.count(WASM_PREFIX) == 1,
           'expected one %r in the bundle; has build.sh changed its loader?' % WASM_PREFIX)
    return text.replace(WASM_PREFIX, BROKEN_WASM_PREFIX)


def before_loader(text, script):
    """The bundle with `script` placed just before the engine loader."""
    expect(text.count(LOADER_TAG) == 1,
           'expected one %r in the bundle; has build.sh changed its loader?' % LOADER_TAG)
    return text.replace(LOADER_TAG, script + LOADER_TAG)


def start_broken(app, edit):
    """Open a temporary, edited copy of the bundle; returns (App, cleanup)."""
    folder = tempfile.TemporaryDirectory(prefix='lrv-startup-')
    try:
        copy = Path(folder.name) / 'index.html'
        copy.write_text(edit(DIST.read_text(encoding='utf-8')), encoding='utf-8')
        other = open_page(app, copy.as_uri(), allowed=LOCAL_SCHEMES, offline=True)
    except BaseException:
        folder.cleanup()
        raise

    def cleanup():
        other.close()
        folder.cleanup()

    return other, cleanup


def startup_failure(app, edit, expect_tag, expect_text, never, booted, allow_console=()):
    """Open an edited bundle and check the one error box it must show."""
    other, cleanup = start_broken(app, edit)
    try:
        other.page.wait_for_selector('#messages .msg.error')
        other.page.wait_for_timeout(200)   # anything else that was going to appear
        boxes = error_boxes(other.page)
        expect(len(boxes) == 1, 'expected one error box, got %r' % boxes)
        box = boxes[0]
        expect(box['tag'] == expect_tag + ': ',
               'the box is tagged %r, not %r' % (box['tag'], expect_tag + ': '))
        for text in expect_text:
            expect(text in box['text'], 'the message does not say %r: %r' % (text, box['text']))
        expect(never not in other.messages(),
               'a %s was reported as %r: %r'
               % ('page failure' if never == ENGINE_FAILURE else 'load failure',
                  never, other.messages()))
        has_ui = other.page.evaluate("() => !!document.getElementById('variant-lr0')")
        expect(has_ui == booted, 'the page %s its controls, which is wrong here'
               % ('built' if has_ui else 'did not build'))
        # Caught and reported, with no request and no dialog.
        problems = other.log.problems(allow_console=allow_console)
        expect(not problems, '; '.join(problems))
        app.note('%s -> "%s"' % (expect_tag, box['text'][:110]))
    finally:
        cleanup()


@test()
def engine_that_cannot_load_is_reported_as_a_load_failure(app):
    """A corrupt engine: 'could not be loaded', through app.js's LRV.bootFailed."""
    startup_failure(app, break_engine, expect_tag='Engine',
                    expect_text=(ENGINE_FAILURE, 'CompileError'),
                    never=PAGE_FAILURE, booted=False)


@test()
def engine_that_cannot_load_is_reported_without_app_js(app):
    """The same fault with app.js gone: the loader builds the message itself."""
    startup_failure(app, lambda text: before_loader(break_engine(text), PAGE_SCRIPTS_GONE),
                    expect_tag='Engine', expect_text=(ENGINE_FAILURE, 'CompileError'),
                    never=PAGE_FAILURE, booted=False)


# The loader logs the page failure it caught, once, under this name.
LOADER_CONSOLE = (r'LR visualizer: internal rendering error',)


@test()
def boot_that_throws_is_not_called_a_load_failure(app):
    """The engine loads and LRV.boot throws: a bug in the visualizer, said so."""
    startup_failure(app, lambda text: before_loader(text, BOOT_THROWS),
                    expect_tag=PAGE_FAILURE,
                    expect_text=('boot sabotaged by the test', PAGE_FAILURE_HINT),
                    never=ENGINE_FAILURE, booted=False, allow_console=LOADER_CONSOLE)


@test()
def missing_page_scripts_are_not_called_a_load_failure(app):
    """The engine loads and there is no LRV at all: still not a load failure."""
    startup_failure(app, lambda text: before_loader(text, PAGE_SCRIPTS_GONE),
                    expect_tag=PAGE_FAILURE, expect_text=(PAGE_FAILURE_HINT,),
                    never=ENGINE_FAILURE, booted=False, allow_console=LOADER_CONSOLE)


# ------------------------------- the same two failures, on the development page

# What the test serves in place of one of the dev page's own files.
BOOT_THROWS_JS = ("\n;window.LRV.boot = function () "
                  "{ throw new Error('boot sabotaged by the test'); };\n")
# A few bytes that are not a WebAssembly module, served as one.
NOT_WASM = b'\x00\x00\x00\x00not-wasm'


def dev_startup_failure(app, answer, expect_tag, expect_text, never, allow_console=()):
    """Serve web/ with one request answered by `answer(route, name)` (True if fulfilled); check the one box."""
    with dev_server() as base:
        def intercept(route, request):
            return request.url.startswith(base) and answer(route, request.url[len(base):])

        dev = open_page(app, base, allowed=(base, 'data:', 'blob:'), offline=False,
                        intercept=intercept)
        try:
            dev.page.wait_for_selector('#messages .msg.error')
            dev.page.wait_for_timeout(200)   # anything else that was going to appear
            boxes = error_boxes(dev.page)
            expect(len(boxes) == 1, 'expected one error box, got %r' % boxes)
            box = boxes[0]
            expect(box['tag'] == expect_tag + ': ',
                   'the box is tagged %r, not %r' % (box['tag'], expect_tag + ': '))
            for text in expect_text:
                expect(text in box['text'],
                       'the message does not say %r: %r' % (text, box['text']))
            expect(never not in dev.messages(),
                   'a %s was reported as %r: %r'
                   % ('page failure' if never == ENGINE_FAILURE else 'load failure',
                      never, dev.messages()))
            expect(not dev.page.evaluate("() => !!document.getElementById('variant-lr0')"),
                   'the page built its controls, which is wrong here')
            # Caught and reported. The Firefox /favicon.ico refusal is set aside, as above.
            expect(not dev.log.page_errors, 'page errors: %r' % dev.log.page_errors)
            allowed = tuple(allow_console) + (re.escape(base + 'favicon.ico'),)
            stray = [t for t in dev.log.console_errors
                     if not any(re.search(p, t) for p in allowed)]
            expect(not stray, 'console errors: %r' % stray)
            expect(not dev.log.dialogs, 'dialogs: %r' % dev.log.dialogs)
            app.note('dev page: %s -> "%s"' % (expect_tag, box['text'][:110]))
        finally:
            dev.close()


@test()
def dev_page_boot_that_throws_is_not_called_a_load_failure(app):
    """web/index.html's own loader: the engine loads, LRV.boot throws, and it says so."""
    def answer(route, name):
        if name != 'app.js':
            return False
        route.fulfill(status=200, content_type='text/javascript',
                      body=(WEB / 'app.js').read_text(encoding='utf-8') + BOOT_THROWS_JS)
        return True

    dev_startup_failure(app, answer, expect_tag=PAGE_FAILURE,
                        expect_text=('boot sabotaged by the test', PAGE_FAILURE_HINT),
                        never=ENGINE_FAILURE, allow_console=LOADER_CONSOLE)


@test()
def dev_page_engine_that_cannot_load_is_a_load_failure(app):
    """The dev page with an engine that does not compile, then with no pkg/ at all."""
    def bad_wasm(route, name):
        if name != 'pkg/lr_engine_bg.wasm':
            return False
        route.fulfill(status=200, content_type='application/wasm', body=NOT_WASM)
        return True

    dev_startup_failure(app, bad_wasm, expect_tag='Engine',
                        expect_text=(ENGINE_FAILURE, 'CompileError'), never=PAGE_FAILURE)

    # pkg/ does not exist: the browser logs the missing module, and the page must still say what happened.
    def no_pkg(route, name):
        if not name.startswith('pkg/'):
            return False
        route.fulfill(status=404, content_type='text/plain', body='not found')
        return True

    dev_startup_failure(app, no_pkg, expect_tag='Engine', expect_text=(ENGINE_FAILURE,),
                        never=PAGE_FAILURE,
                        allow_console=(r'lr_engine', r'Failed to load resource', r'404',
                                       r'MIME', r'module'))


@test()
def untouched_bundle_starts_with_no_error_box(app):
    """Control for the four above: the same checks on the real file find nothing."""
    expect(error_boxes(app.page) == [], 'the real bundle shows an error: %r'
           % error_boxes(app.page))
    expect(app.page.evaluate("() => !!document.getElementById('variant-lr0')"),
           'the real bundle did not build its controls')
    text = DIST.read_text(encoding='utf-8')
    expect(text.count(WASM_PREFIX) == 1 and text.count(LOADER_TAG) == 1,
           'the bundle no longer has the two anchors the start-up tests edit')


if __name__ == '__main__':
    raise SystemExit(main())
