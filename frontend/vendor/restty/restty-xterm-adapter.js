// restty xterm.js adapter for Den
// Extends restty's xterm compat Terminal with stub methods that Den requires
// but restty does not implement (OSC handlers, title change, selection, etc.)

import { Terminal as ResttyTerminal } from './xterm.js';
import { parseGhosttyTheme } from './restty.js';

const textDecoder = new TextDecoder();

function noopDisposable() {
  return { dispose() {} };
}

/**
 * Convert xterm.js theme object to Ghostty theme string format.
 * xterm.js: { background: '#1a1b26', foreground: '#c0caf5', black: '#15161e', ... }
 * Ghostty:  "background = 1a1b26\nforeground = c0caf5\npalette = 0=#15161e\n..."
 */
const ANSI_COLOR_KEYS = [
  'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
  'brightBlack', 'brightRed', 'brightGreen', 'brightYellow',
  'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite',
];

function xtermThemeToGhostty(theme) {
  if (!theme) return null;
  const lines = [];
  if (theme.background) lines.push(`background = ${theme.background.replace('#', '')}`);
  if (theme.foreground) lines.push(`foreground = ${theme.foreground.replace('#', '')}`);
  if (theme.cursor) lines.push(`cursor-color = ${theme.cursor.replace('#', '')}`);
  if (theme.selectionBackground) lines.push(`selection-background = ${theme.selectionBackground.replace('#', '')}`);
  if (theme.selectionForeground) lines.push(`selection-foreground = ${theme.selectionForeground.replace('#', '')}`);
  for (let i = 0; i < ANSI_COLOR_KEYS.length; i++) {
    const val = theme[ANSI_COLOR_KEYS[i]];
    if (val) lines.push(`palette = ${i}=${val.replace('#', '')}`);
  }
  return lines.join('\n');
}

/**
 * Convert CSS fontFamily string to restty fontSources array.
 * CSS: '"Cascadia Code", "Fira Code", monospace'
 * restty: [{ type: "local", matchers: ["cascadia code"], label: "Cascadia Code" }, ...]
 */
/** CDN fallback fonts — used when no local font is available (e.g. iPad) */
const CDN_FONT_FALLBACKS = [
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Regular/JetBrainsMonoNLNerdFontMono-Regular.ttf', label: 'JetBrains Mono NL Nerd Font Regular (CDN)' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Bold/JetBrainsMonoNLNerdFontMono-Bold.ttf', label: 'JetBrains Mono NL Nerd Font Bold (CDN)' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Italic/JetBrainsMonoNLNerdFontMono-Italic.ttf', label: 'JetBrains Mono NL Nerd Font Italic (CDN)' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/BoldItalic/JetBrainsMonoNLNerdFontMono-BoldItalic.ttf', label: 'JetBrains Mono NL Nerd Font Bold Italic (CDN)' },
];

function fontFamilyToSources(fontFamily) {
  if (!fontFamily) return undefined;
  const sources = [];
  const families = fontFamily.split(',').map(f => f.trim().replace(/^["']|["']$/g, ''));
  for (const family of families) {
    const lower = family.toLowerCase();
    if (lower === 'monospace' || lower === 'sans-serif' || lower === 'serif') continue;
    sources.push({ type: 'local', matchers: [lower], label: family });
  }
  // Append CDN fallbacks so restty can render even when no local fonts exist
  sources.push(...CDN_FONT_FALLBACKS);
  return sources.length > 0 ? sources : undefined;
}

function applyResttyTheme(restty, theme) {
  const ghosttyTheme = xtermThemeToGhostty(theme);
  if (!ghosttyTheme) return;
  try {
    const parsed = parseGhosttyTheme(ghosttyTheme);
    restty.applyTheme(parsed, 'inline');
  } catch (e) {
    console.warn('[restty] Failed to apply theme:', e);
  }
}

/**
 * DenTerminal adapter — wraps restty's xterm compat Terminal with
 * additional stubs for APIs Den uses but restty doesn't provide.
 *
 * WASM buffering: restty's sendInput() silently drops data when WASM is
 * not yet initialized. Since app.init() is async and not awaited by the
 * pane manager, early writes (including ConPTY's DSR probe) are lost.
 * We buffer writes until onBackend fires (GPU init done) and the WASM
 * promise has resolved, then replay the buffered data.
 */
class DenResttyTerminal extends ResttyTerminal {
  /** Stub parser with registerOscHandler */
  parser = {
    registerOscHandler(_id, _handler) {
      return noopDisposable();
    },
  };

  /** Stub buffer object for select mode coordinate conversion */
  buffer = {
    active: {
      get viewportY() { return 0; },
    },
  };

  /** Pending theme to apply after open() */
  _pendingTheme = null;

  /** Write buffer for data arriving before WASM is ready */
  _writeQueue = [];
  _wasmReady = false;

  constructor(options = {}) {
    // Extract theme and fontFamily before passing to restty
    const { theme, fontFamily, ...restOptions } = options;
    const fontSources = fontFamilyToSources(fontFamily);
    super({
      ...restOptions,
      appOptions: {
        renderer: 'auto',
        autoResize: true,
        fontSize: options.fontSize || 15,
        touchSelectionMode: 'long-press',
        ...(fontSources ? { fontSources } : {}),
      },
    });

    // Inject onBackend callback into stored appOptions.
    // onBackend fires during app.init() after GPU renderer is determined.
    // WASM loading runs in parallel and is awaited right after onBackend,
    // so we schedule flush after microtasks + one animation frame.
    const origAppOptions = this.userAppOptions;
    this.userAppOptions = {
      ...origAppOptions,
      callbacks: {
        ...origAppOptions?.callbacks,
        onBackend: () => this._onBackendReady(),
      },
    };

    if (theme) {
      this._pendingTheme = theme;
    }
  }

  _onBackendReady() {
    // onBackend fires, then init() does `await wasmPromise`.
    // setTimeout(0) runs after microtasks (including the await resolution).
    // requestAnimationFrame ensures we're past the render loop start.
    setTimeout(() => {
      requestAnimationFrame(() => {
        this._flushWrites();
      });
    }, 0);
  }

  _flushWrites() {
    this._wasmReady = true;
    const queue = this._writeQueue;
    this._writeQueue = [];
    for (const data of queue) {
      super.write(data);
    }
  }

  open(parent) {
    super.open(parent);
    // Apply theme after restty instance is created
    if (this._pendingTheme && this.restty) {
      applyResttyTheme(this.restty, this._pendingTheme);
      this._pendingTheme = null;
    }

    // Fallback: if onBackend never fires (shouldn't happen, but be safe),
    // flush after 3 seconds regardless.
    setTimeout(() => {
      if (!this._wasmReady) {
        console.warn('[restty] WASM ready timeout — flushing write buffer');
        this._flushWrites();
      }
    }, 3000);
  }

  /**
   * Override write to handle Uint8Array and buffer before WASM is ready.
   * restty's sendInput expects string, so decode if needed.
   */
  write(data, callback) {
    if (data instanceof Uint8Array || data instanceof ArrayBuffer) {
      data = textDecoder.decode(data instanceof ArrayBuffer ? new Uint8Array(data) : data);
    }
    if (!this._wasmReady) {
      this._writeQueue.push(data);
      callback?.();
      return;
    }
    super.write(data, callback);
  }

  /** Override options setter to intercept theme and font changes */
  get options() {
    return super.options;
  }

  set options(next) {
    if (next && next.theme && this.restty) {
      applyResttyTheme(this.restty, next.theme);
    }
    if (next && next.fontSize && this.restty) {
      try {
        this.restty.setFontSize(next.fontSize);
      } catch (_) { /* may not be supported */ }
    }
    if (next && next.fontFamily && this.restty) {
      const fontSources = fontFamilyToSources(next.fontFamily);
      if (fontSources) {
        try {
          this.restty.setFontSources(fontSources);
        } catch (_) { /* may not be supported */ }
      }
    }
    // Store all option values via parent
    super.options = next;
  }

  // --- Stub methods for Den-specific APIs ---

  onBinary(_cb) { return noopDisposable(); }
  onTitleChange(_cb) { return noopDisposable(); }
  attachCustomKeyEventHandler(_handler) { /* noop */ }

  getSelection() { return ''; }
  select(_col, _row, _length) { /* noop */ }
  clearSelection() { /* noop */ }

  refresh(_start, _end) { /* noop — restty manages its own rendering */ }
}

/** No-op FitAddon — restty has built-in auto-resize */
class NoopFitAddon {
  activate() {}
  dispose() {}
  fit() {}
  proposeDimensions() { return null; }
}

export { DenResttyTerminal, NoopFitAddon, xtermThemeToGhostty };
