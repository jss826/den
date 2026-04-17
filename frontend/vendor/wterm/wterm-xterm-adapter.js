// wterm adapter for Den — wraps WTerm with xterm.js-compatible API surface.
// Runs alongside xterm.js and restty; selected via DenSettings.terminal_renderer.

import { WTerm } from './wterm.bundle.js';

const textDecoder = new TextDecoder('utf-8', { fatal: false });

let _cssInjected = false;
function ensureCss() {
  if (_cssInjected) return;
  _cssInjected = true;
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = '/vendor/wterm/wterm.css?v=13';
  document.head.appendChild(link);
}

function noopDisposable() {
  return { dispose() {} };
}

/**
 * Map an xterm-style theme object to wterm CSS custom properties.
 * wterm uses `--term-bg`, `--term-fg`, `--term-cursor`, and `--term-color-0..15`.
 */
const ANSI_COLOR_KEYS = [
  'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
  'brightBlack', 'brightRed', 'brightGreen', 'brightYellow',
  'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite',
];

function applyThemeToElement(el, theme) {
  if (!el || !theme) return;
  const s = el.style;
  if (theme.background) s.setProperty('--term-bg', theme.background);
  if (theme.foreground) s.setProperty('--term-fg', theme.foreground);
  if (theme.cursor) s.setProperty('--term-cursor', theme.cursor);
  for (let i = 0; i < ANSI_COLOR_KEYS.length; i++) {
    const val = theme[ANSI_COLOR_KEYS[i]];
    if (val) s.setProperty(`--term-color-${i}`, val);
  }
}

function applyFontToElement(el, fontFamily, fontSize) {
  if (!el) return;
  const s = el.style;
  if (fontFamily) s.setProperty('--term-font-family', fontFamily);
  if (fontSize) s.setProperty('--term-font-size', `${fontSize}px`);
}

/**
 * DenWtermTerminal — xterm.js-compatible facade over `WTerm`.
 * Defers WTerm construction until `open(parent)` to match xterm.js lifecycle
 * (`new Terminal(options)` then `term.open(el)`).
 */
class DenWtermTerminal {
  parser = { registerOscHandler(_id, _handler) { return noopDisposable(); } };
  buffer = { active: { get viewportY() { return 0; } } };

  cols = 80;
  rows = 24;

  constructor(options = {}) {
    const { theme, fontFamily, fontSize, cursorBlink } = options;
    this._theme = theme || null;
    this._fontFamily = fontFamily || null;
    this._fontSize = fontSize || 15;
    this._cursorBlink = cursorBlink !== false;
    this._dataListeners = new Set();
    this._resizeListeners = new Set();
    this._titleListeners = new Set();
    this._wterm = null;
    this._element = null;
    this._inner = null;
    this._parentObserver = null;
    this._externalTextarea = null;
    this._ready = false;
    this._writeQueue = [];
    this._disposed = false;
  }

  get element() { return this._element; }
  get wterm() { return this._wterm; }

  open(parent) {
    if (this._wterm) throw new Error('Already opened');
    ensureCss();
    this._element = parent;

    // `.wterm` must be viewport-sized so `has-scrollback { overflow-y: auto }`
    // scrolls when scrollback piles up. Use an inner wrapper that follows
    // the parent's layout-assigned height via CSS (100%). This keeps Den's
    // dynamic height management — including iOS virtual keyboard — intact,
    // because we never pin `parent.style.height`.
    if (!parent.style.position || parent.style.position === 'static') {
      parent.style.position = 'relative';
    }
    const inner = document.createElement('div');
    inner.style.position = 'absolute';
    inner.style.inset = '0';
    inner.style.boxSizing = 'border-box';
    parent.appendChild(inner);
    this._inner = inner;

    applyFontToElement(inner, this._fontFamily, this._fontSize);
    if (this._theme) applyThemeToElement(inner, this._theme);

    // autoResize: false — we drive resizes from the parent's ResizeObserver
    // so scrollback growth inside `.wterm` doesn't retrigger cols/rows.
    const wterm = new WTerm(inner, {
      cols: this.cols,
      rows: this.rows,
      autoResize: false,
      cursorBlink: this._cursorBlink,
      onData: (data) => {
        for (const cb of this._dataListeners) {
          try { cb(data); } catch (e) { /* ignore */ }
        }
      },
      onResize: (cols, rows) => {
        if (cols !== this.cols || rows !== this.rows) {
          this.cols = cols;
          this.rows = rows;
          for (const cb of this._resizeListeners) {
            try { cb({ cols, rows }); } catch (e) { /* ignore */ }
          }
        }
      },
      onTitle: (title) => {
        for (const cb of this._titleListeners) {
          try { cb(title); } catch (e) { /* ignore */ }
        }
      },
    });

    this._wterm = wterm;

    wterm.init().then(() => {
      if (this._disposed) return;
      // WTerm's `_lockHeight()` (called when autoResize is off) hard-codes
      // element.style.height to `rows * rowHeight + padding`, which overrides
      // our `inset: 0`. Clear it so the wrapper stays parent-sized.
      inner.style.height = '';
      this._ready = true;
      this.cols = wterm.cols;
      this.rows = wterm.rows;
      this._relocateTextarea();
      this._setupParentObserver(parent);
      this._relayoutFromParent();
      const queue = this._writeQueue;
      this._writeQueue = [];
      for (const data of queue) {
        try { wterm.write(data); } catch (e) { /* ignore */ }
      }
    }).catch((e) => {
      console.error('[wterm] init failed:', e);
    });
  }

  /** Move the hidden input textarea out of the scroll container.
   *  iOS Safari scrolls its ancestor to reveal the focused field, which
   *  yanks `.wterm` to scrollTop=0 (or bottom) on every focus. Reparenting to
   *  `document.body` decouples focus from the terminal's scroll position.
   *  Also bumps font-size past the iOS auto-zoom threshold (16px). */
  _relocateTextarea() {
    const ta = this._wterm?.input?.textarea;
    if (!ta) return;
    document.body.appendChild(ta);
    // Place just above the software keyboard position (bottom: 0). A textarea
    // pinned at `top: 0` invites iOS Safari to scroll the page up on focus,
    // making the terminal content shift off-screen.
    ta.style.position = 'fixed';
    ta.style.left = '0';
    ta.style.bottom = '0';
    ta.style.top = 'auto';
    ta.style.width = '1px';
    ta.style.height = '1px';
    ta.style.opacity = '0';
    ta.style.pointerEvents = 'none';
    ta.style.zIndex = '-1';
    ta.style.fontSize = '16px';
    // Prevent scrollIntoView from scrolling the page on focus.
    ta.style.scrollMargin = '0';
    this._externalTextarea = ta;

    // Belt-and-suspenders: reset window scroll when the textarea gains focus,
    // in case iOS Safari still scrolls html despite the positioning above.
    this._onTextareaFocus = () => {
      requestAnimationFrame(() => {
        if (window.scrollY !== 0) window.scrollTo(0, 0);
        if (document.documentElement.scrollTop !== 0) document.documentElement.scrollTop = 0;
      });
    };
    ta.addEventListener('focus', this._onTextareaFocus);
    // Intercept Shift+PageUp/PageDown/Home/End for scrollback, matching xterm.js.
    // Capture phase so we run before wterm's keydown handler sends to PTY.
    this._onScrollKey = (e) => {
      if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
      const inner = this._inner;
      if (!inner) return;
      const page = Math.max(1, Math.floor(inner.clientHeight * 0.9));
      let handled = true;
      if (e.key === 'PageUp') inner.scrollTop = Math.max(0, inner.scrollTop - page);
      else if (e.key === 'PageDown') inner.scrollTop = Math.min(inner.scrollHeight, inner.scrollTop + page);
      else if (e.key === 'Home') inner.scrollTop = 0;
      else if (e.key === 'End') inner.scrollTop = inner.scrollHeight;
      else handled = false;
      if (handled) { e.preventDefault(); e.stopImmediatePropagation(); }
    };
    ta.addEventListener('keydown', this._onScrollKey, true);
  }

  _setupParentObserver(parent) {
    if (this._parentObserver) return;
    this._parentObserver = new ResizeObserver(() => this._relayoutFromParent());
    this._parentObserver.observe(parent);
  }

  /** Measure the inner box, resize WTerm's grid to fit. `.wterm` height
   *  inherits from the inner wrapper (100% of parent), which Den manages
   *  dynamically — so iOS keyboard / window resize propagate for free. */
  _relayoutFromParent() {
    const inner = this._inner;
    const wterm = this._wterm;
    if (!inner || !wterm) return;
    const cs = getComputedStyle(inner);
    const padX = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
    const padY = (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0);
    const width = inner.clientWidth - padX;
    const height = inner.clientHeight - padY;
    if (width <= 0 || height <= 0) return;

    const probe = document.createElement('span');
    probe.textContent = 'W';
    probe.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;';
    inner.appendChild(probe);
    const pr = probe.getBoundingClientRect();
    const charW = pr.width;
    const charH = pr.height;
    probe.remove();
    if (!charW || !charH) return;

    const cols = Math.max(1, Math.floor(width / charW));
    const rows = Math.max(1, Math.floor(height / charH));
    // Capture scroll state before any resize so we can restore or stick to
    // bottom afterwards. The viewport may have shrunk (soft keyboard) even
    // if cols/rows didn't change, so always reconcile below.
    const prevScrollTop = inner.scrollTop;
    const atBottom = inner.scrollTop + inner.clientHeight >= inner.scrollHeight - 5;
    if (cols !== this.cols || rows !== this.rows) {
      try { wterm.resize(cols, rows); } catch (_) {}
    }
    requestAnimationFrame(() => {
      if (atBottom) {
        inner.scrollTop = inner.scrollHeight;
      } else if (prevScrollTop > 0 && inner.scrollTop !== prevScrollTop) {
        inner.scrollTop = prevScrollTop;
      }
    });
    // Force overflow so Den's container CSS doesn't win on ID specificity.
    if (inner.style.overflowY !== 'auto') inner.style.overflowY = 'auto';
    if (inner.style.overflowX !== 'hidden') inner.style.overflowX = 'hidden';
  }

  write(data, callback) {
    // ConPTY DSR probe: reply CPR via onData so the server-side PTY unblocks.
    if (typeof data === 'string' && data.includes('\x1b[6n')) {
      const cpr = '\x1b[1;1R';
      for (const cb of this._dataListeners) {
        try { cb(cpr); } catch (e) { /* ignore */ }
      }
    }
    if (data instanceof ArrayBuffer) data = new Uint8Array(data);
    if (data instanceof Uint8Array) {
      // Strip BEL (0x07) to avoid OS audio beep on shell completion failures,
      // matching xterm.js default (`bellStyle: 'none'`).
      if (data.includes(7)) data = data.filter((b) => b !== 7);
    } else if (typeof data === 'string') {
      if (data.includes('\x07')) data = data.replace(/\x07/g, '');
    } else {
      data = String(data);
    }

    if (!this._ready) {
      this._writeQueue.push(data);
      callback?.();
      return;
    }
    try { this._wterm?.write(data); } catch (e) { console.warn('[wterm] write failed:', e); }
    callback?.();
  }

  writeln(data = '', callback) {
    // wterm.write accepts string only for writeln's concatenation path.
    if (data instanceof Uint8Array || data instanceof ArrayBuffer) {
      data = textDecoder.decode(data instanceof ArrayBuffer ? new Uint8Array(data) : data, { stream: true });
    }
    this.write(`${data}\r\n`, callback);
  }

  // --- Event listeners (xterm.js compatible) ---

  onData(cb) {
    this._dataListeners.add(cb);
    return { dispose: () => this._dataListeners.delete(cb) };
  }

  onResize(cb) {
    this._resizeListeners.add(cb);
    return { dispose: () => this._resizeListeners.delete(cb) };
  }

  onTitleChange(cb) {
    this._titleListeners.add(cb);
    return { dispose: () => this._titleListeners.delete(cb) };
  }

  onBinary(_cb) { return noopDisposable(); }

  // --- Terminal control ---

  focus() {
    const inner = this._inner;
    const prev = inner?.scrollTop;
    this._wterm?.focus();
    if (inner != null && prev != null) {
      requestAnimationFrame(() => {
        if (inner.scrollTop !== prev) inner.scrollTop = prev;
      });
    }
  }
  blur() { this._inner?.blur?.(); }

  clear() { this.write('\x1b[2J\x1b[H'); }

  reset() { this.write('\x1bc'); }

  resize(cols, rows) {
    this.cols = cols;
    this.rows = rows;
    try { this._wterm?.resize(cols, rows); } catch (_) { /* before init */ }
  }

  // --- Options (theme, font) ---

  get options() {
    const self = this;
    return new Proxy({ fontSize: this._fontSize }, {
      set(_target, prop, value) {
        if (prop === 'theme') {
          self._theme = value;
          applyThemeToElement(self._inner, value);
        } else if (prop === 'fontSize') {
          self._fontSize = value;
          applyFontToElement(self._inner, null, value);
          self._relayoutFromParent();
        } else if (prop === 'scrollback') {
          // wterm manages scrollback internally and exposes no API
        }
        return true;
      },
    });
  }

  set options(next) {
    if (!next) return;
    if (next.theme) {
      this._theme = next.theme;
      applyThemeToElement(this._inner, next.theme);
    }
    if (next.fontSize) {
      this._fontSize = next.fontSize;
      applyFontToElement(this._inner, null, next.fontSize);
      this._relayoutFromParent();
    }
    if (next.fontFamily) {
      this._fontFamily = next.fontFamily;
      applyFontToElement(this._inner, next.fontFamily, null);
      this._relayoutFromParent();
    }
  }

  // --- Stubs matching xterm.js API consumed by Den ---

  loadAddon(addon) { addon?.activate?.(this); }
  attachCustomKeyEventHandler(_handler) {}
  getSelection() {
    try { return document.getSelection()?.toString() ?? ''; } catch (_) { return ''; }
  }
  select(_col, _row, _length) {}
  clearSelection() { try { document.getSelection()?.removeAllRanges(); } catch (_) {} }
  refresh(_start, _end) {}
  setOption(_key, _value) {}
  getOption(key) {
    if (key === 'cols') return this.cols;
    if (key === 'rows') return this.rows;
    return undefined;
  }

  dispose() {
    if (this._disposed) return;
    this._disposed = true;
    this._writeQueue.length = 0;
    this._dataListeners.clear();
    this._resizeListeners.clear();
    this._titleListeners.clear();
    try { this._parentObserver?.disconnect(); } catch (_) {}
    try {
      if (this._externalTextarea) {
        if (this._onScrollKey) this._externalTextarea.removeEventListener('keydown', this._onScrollKey, true);
        if (this._onTextareaFocus) this._externalTextarea.removeEventListener('focus', this._onTextareaFocus);
      }
      this._externalTextarea?.remove();
    } catch (_) {}
    try { this._wterm?.destroy(); } catch (_) {}
    try { this._inner?.remove(); } catch (_) {}
    this._wterm = null;
    this._element = null;
    this._inner = null;
    this._parentObserver = null;
    this._externalTextarea = null;
  }
}

/** FitAddon stub — delegates to DenWtermTerminal._relayoutFromParent so that
 *  Den's visualViewport handler (app.js → DenTerminal.scheduleFit) can drive
 *  wterm resize when the iOS soft keyboard appears/disappears. */
class NoopFitAddon {
  _term = null;
  activate(term) { this._term = term; }
  dispose() { this._term = null; }
  fit() { this._term?._relayoutFromParent?.(); }
  proposeDimensions() {
    const t = this._term;
    if (!t) return null;
    return { cols: t.cols, rows: t.rows };
  }
}

export { DenWtermTerminal, NoopFitAddon, applyThemeToElement, applyFontToElement };
