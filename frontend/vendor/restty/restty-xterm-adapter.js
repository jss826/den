// restty adapter for Den — uses Restty class directly (bypasses xterm compat layer)
// The xterm.js compat layer's emitData causes input duplication. By using the
// Restty class directly with a custom ptyTransport, all input (keyboard, paste,
// terminal replies) flows through a single path: ptyTransport.sendInput → onData.

import { Restty, parseGhosttyTheme } from './restty.js';

const textDecoder = new TextDecoder('utf-8', { fatal: false });

function noopDisposable() {
  return { dispose() {} };
}

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

const CDN_FONT_FALLBACKS = [
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Regular/JetBrainsMonoNLNerdFontMono-Regular.ttf', label: 'JetBrains Mono NL Nerd Font Regular (CDN)' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Bold/JetBrainsMonoNLNerdFontMono-Bold.ttf', label: 'JetBrains Mono NL Nerd Font Bold (CDN)' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Italic/JetBrainsMonoNLNerdFontMono-Italic.ttf', label: 'JetBrains Mono NL Nerd Font Italic (CDN)' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/BoldItalic/JetBrainsMonoNLNerdFontMono-BoldItalic.ttf', label: 'JetBrains Mono NL Nerd Font Bold Italic (CDN)' },
];

/** Common Nerd Font local matchers — tried before CDN fallback */
const NERD_FONT_LOCAL = [
  { type: 'local', matchers: ['jetbrainsmono nerd font', 'jetbrains mono nerd font', 'jetbrains mono nl nerd font mono'], label: 'JetBrains Mono Nerd Font (Local)' },
  { type: 'local', matchers: ['fira code nerd font', 'firacode nerd font'], label: 'Fira Code Nerd Font (Local)' },
  { type: 'local', matchers: ['hack nerd font'], label: 'Hack Nerd Font (Local)' },
  { type: 'local', matchers: ['meslo lgm nerd font', 'meslo nerd font'], label: 'Meslo Nerd Font (Local)' },
];

function fontFamilyToSources(fontFamily) {
  const sources = [];
  // 1. User-configured fonts from Den settings (local)
  if (fontFamily) {
    const families = fontFamily.split(',').map(f => f.trim().replace(/^["']|["']$/g, ''));
    for (const family of families) {
      const lower = family.toLowerCase();
      if (lower === 'monospace' || lower === 'sans-serif' || lower === 'serif') continue;
      sources.push({ type: 'local', matchers: [lower], label: family });
    }
  }
  // 2. Common Nerd Fonts (local)
  sources.push(...NERD_FONT_LOCAL);
  // 3. CDN fallback — guaranteed to work even without local fonts
  sources.push(...CDN_FONT_FALLBACKS);
  return sources.length > 0 ? sources : undefined;
}

/**
 * DenResttyTerminal — wraps Restty class with xterm.js-compatible API surface.
 * Uses a custom ptyTransport so all input flows through a single path.
 */
class DenResttyTerminal {
  // xterm.js API stubs
  parser = { registerOscHandler(_id, _handler) { return noopDisposable(); } };
  buffer = { active: { get viewportY() { return 0; } } };

  cols = 80;
  rows = 24;

  constructor(options = {}) {
    const { theme, fontFamily, fontSize, scrollback, cursorBlink, ...rest } = options;
    this._fontSize = fontSize || 15;
    this._fontSources = fontFamilyToSources(fontFamily);
    this._theme = theme;
    this._dataListeners = new Set();
    this._resizeListeners = new Set();
    this._restty = null;
    this._element = null;
    this._disposed = false;
    this._writeQueue = [];
    this._wasmReady = false;
  }

  get element() { return this._element; }
  get restty() { return this._restty; }

  open(parent) {
    if (this._restty) throw new Error('Already opened');
    this._element = parent;

    const dataListeners = this._dataListeners;

    // Don't pass custom fontSources. Restty's DEFAULT_FONT_SOURCES includes
    // JetBrains Mono Nerd Font (CDN), Symbols Nerd Font, Noto Sans CJK (CDN),
    // Noto Color Emoji, etc. Custom fontSources would replace ALL of these.
    this._restty = new Restty({
      root: parent,
      appOptions: () => ({
        renderer: 'auto',
        autoResize: true,
        fontSize: this._fontSize,
        touchSelectionMode: 'long-press',
        ptyTransport: {
          connect() {},
          disconnect() {},
          sendInput(data) {
            for (const cb of dataListeners) {
              try { cb(data); } catch (e) { /* ignore */ }
            }
          },
          resize() {},
          isConnected: () => true,
          destroy() {},
        },
        callbacks: {
          onGridSize: (cols, rows) => {
            if (cols !== this.cols || rows !== this.rows) {
              this.cols = cols;
              this.rows = rows;
              for (const cb of this._resizeListeners) {
                try { cb({ cols, rows }); } catch (e) { /* ignore */ }
              }
            }
          },
          onBackend: () => this._onBackendReady(),
        },
      }),
    });

    // Apply initial theme
    if (this._theme) {
      const ghosttyTheme = xtermThemeToGhostty(this._theme);
      if (ghosttyTheme) {
        try {
          const parsed = parseGhosttyTheme(ghosttyTheme);
          this._restty.applyTheme(parsed, 'inline');
        } catch (e) {
          console.warn('[restty] Failed to apply theme:', e);
        }
      }
    }

    // Fallback: flush writes after 3s if onBackend never fires
    setTimeout(() => {
      if (!this._wasmReady) {
        console.warn('[restty] WASM ready timeout — flushing write buffer');
        this._flushWrites();
      }
    }, 3000);
  }

  _onBackendReady() {
    setTimeout(() => {
      requestAnimationFrame(() => this._flushWrites());
    }, 0);
  }

  _flushWrites() {
    this._wasmReady = true;
    const queue = this._writeQueue;
    this._writeQueue = [];
    for (const data of queue) {
      this._restty?.sendInput(data, 'pty');
    }
  }

  write(data, callback) {
    if (data instanceof Uint8Array || data instanceof ArrayBuffer) {
      data = textDecoder.decode(data instanceof ArrayBuffer ? new Uint8Array(data) : data, { stream: true });
    }

    // ConPTY DSR probe: send CPR response through onData → WebSocket → PTY.
    if (typeof data === 'string' && data.includes('\x1b[6n')) {
      const cpr = '\x1b[1;1R';
      for (const cb of this._dataListeners) {
        try { cb(cpr); } catch (e) { /* ignore */ }
      }
    }

    if (!this._wasmReady) {
      this._writeQueue.push(data);
      callback?.();
      return;
    }
    this._restty?.sendInput(data, 'pty');
    callback?.();
  }

  writeln(data = '', callback) {
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

  onBinary(_cb) { return noopDisposable(); }
  onTitleChange(_cb) { return noopDisposable(); }

  // --- Terminal control ---

  focus() { this._restty?.focus(); }
  blur() { this._restty?.blur(); }

  clear() { this._restty?.clearScreen(); }

  reset() {
    this.clear();
    this._restty?.sendInput('\x1bc', 'pty');
  }

  resize(cols, rows) {
    this.cols = cols;
    this.rows = rows;
    this._restty?.resize(cols, rows);
  }

  // --- Options (theme, font) ---

  get options() {
    return { fontSize: this._fontSize };
  }

  set options(next) {
    if (!next) return;
    if (next.theme) {
      const ghosttyTheme = xtermThemeToGhostty(next.theme);
      if (ghosttyTheme) {
        try {
          const parsed = parseGhosttyTheme(ghosttyTheme);
          this._restty?.applyTheme(parsed, 'inline');
        } catch (_) {}
      }
    }
    if (next.fontSize) {
      this._fontSize = next.fontSize;
      try { this._restty?.setFontSize(next.fontSize); } catch (_) {}
    }
    if (next.fontFamily) {
      const fontSources = fontFamilyToSources(next.fontFamily);
      if (fontSources) {
        try { this._restty?.setFontSources(fontSources); } catch (_) {}
      }
    }
  }

  // --- Stubs for Den APIs ---

  loadAddon(addon) { addon?.activate?.(this); }
  attachCustomKeyEventHandler(_handler) {}
  getSelection() { return ''; }
  select(_col, _row, _length) {}
  clearSelection() {}
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
    if (this._restty) {
      this._restty.destroy();
      this._restty = null;
    }
    this._element = null;
  }
}

/** No-op FitAddon — restty has built-in auto-resize */
class NoopFitAddon {
  activate() {}
  dispose() {}
  fit() {}
  proposeDimensions() { return null; }
}

export { DenResttyTerminal, NoopFitAddon, xtermThemeToGhostty };
