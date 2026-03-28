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

/**
 * Replicate restty's DEFAULT_FONT_SOURCES fallback chain so that custom
 * user fonts can be prepended WITHOUT losing CJK, symbol, and emoji support.
 * Source: restty chunk-meqn8xtd.js DEFAULT_FONT_SOURCES
 */
const RESTTY_FALLBACK_SOURCES = [
  // Main monospace — local Nerd Fonts
  { type: 'local', matchers: ['jetbrainsmono nerd font', 'jetbrains mono nerd font', 'jetbrains mono nl nerd font mono', 'jetbrains mono', 'jetbrainsmono'], label: 'JetBrains Mono Nerd Font Regular (Local)' },
  { type: 'local', matchers: ['jetbrainsmono nerd font bold', 'jetbrains mono nerd font bold', 'jetbrains mono nl nerd font mono bold', 'jetbrains mono bold', 'jetbrainsmono bold'], label: 'JetBrains Mono Nerd Font Bold (Local)' },
  { type: 'local', matchers: ['jetbrainsmono nerd font italic', 'jetbrains mono nerd font italic', 'jetbrains mono nl nerd font mono italic', 'jetbrains mono italic', 'jetbrainsmono italic'], label: 'JetBrains Mono Nerd Font Italic (Local)' },
  { type: 'local', matchers: ['jetbrainsmono nerd font bold italic', 'jetbrains mono nerd font bold italic', 'jetbrains mono nl nerd font mono bold italic', 'jetbrains mono bold italic', 'jetbrainsmono bold italic'], label: 'JetBrains Mono Nerd Font Bold Italic (Local)' },
  // Main monospace — CDN fallback
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Regular/JetBrainsMonoNLNerdFontMono-Regular.ttf', label: 'JetBrains Mono Nerd Font Regular' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Bold/JetBrainsMonoNLNerdFontMono-Bold.ttf', label: 'JetBrains Mono Nerd Font Bold' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/Italic/JetBrainsMonoNLNerdFontMono-Italic.ttf', label: 'JetBrains Mono Nerd Font Italic' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/NoLigatures/BoldItalic/JetBrainsMonoNLNerdFontMono-BoldItalic.ttf', label: 'JetBrains Mono Nerd Font Bold Italic' },
  // Symbols — Nerd Font icons
  { type: 'local', matchers: ['symbols nerd font mono', 'symbols nerd font', 'nerd fonts symbols', 'nerdfontssymbolsonly'], label: 'Symbols Nerd Font (Local)' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/NerdFontsSymbolsOnly/SymbolsNerdFontMono-Regular.ttf' },
  // Symbols — general
  { type: 'local', matchers: ['apple symbols', 'applesymbols', 'apple symbols regular'], label: 'Apple Symbols' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/notofonts/noto-fonts@main/unhinted/ttf/NotoSansSymbols2/NotoSansSymbols2-Regular.ttf' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/ChiefMikeK/ttf-symbola@master/Symbola.ttf' },
  // Canadian Aboriginal
  { type: 'local', matchers: ['noto sans canadian aboriginal', 'notosanscanadianaboriginal', 'euphemia ucas', 'euphemiaucas'], label: 'Noto Sans Canadian Aboriginal / Euphemia UCAS' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/notofonts/noto-fonts@main/unhinted/ttf/NotoSansCanadianAboriginal/NotoSansCanadianAboriginal-Regular.ttf', label: 'Noto Sans Canadian Aboriginal' },
  // Emoji
  { type: 'local', matchers: ['apple color emoji', 'applecoloremoji'], label: 'Apple Color Emoji' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/googlefonts/noto-emoji@main/fonts/NotoColorEmoji.ttf' },
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/hfg-gmuend/openmoji@master/font/OpenMoji-black-glyf/OpenMoji-black-glyf.ttf' },
  // CJK
  { type: 'url', url: 'https://cdn.jsdelivr.net/gh/notofonts/noto-cjk@main/Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf' },
];

/** CDN font entries for selectable fonts (setting: restty_font) */
const CDN_FONT_MAP = {
  noto: [
    { type: 'local', matchers: ['noto sans mono', 'notosansmono'], label: 'Noto Sans Mono (Local)' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/notofonts/noto-fonts@main/unhinted/ttf/NotoSansMono/NotoSansMono-Regular.ttf', label: 'Noto Sans Mono Regular' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/notofonts/noto-fonts@main/unhinted/ttf/NotoSansMono/NotoSansMono-Bold.ttf', label: 'Noto Sans Mono Bold' },
  ],
  firacode: [
    { type: 'local', matchers: ['fira code', 'firacode'], label: 'Fira Code (Local)' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/tonsky/FiraCode@6.2/distr/ttf/FiraCode-Regular.ttf', label: 'Fira Code Regular' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/tonsky/FiraCode@6.2/distr/ttf/FiraCode-Bold.ttf', label: 'Fira Code Bold' },
  ],
  cascadia: [
    { type: 'local', matchers: ['cascadia code', 'cascadiacode'], label: 'Cascadia Code (Local)' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/microsoft/cascadia-code@v2404.23/ttf/CascadiaCode.ttf', label: 'Cascadia Code Regular' },
  ],
  iosevka: [
    { type: 'local', matchers: ['iosevka', 'iosevka fixed'], label: 'Iosevka (Local)' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/nicholasgasior/iosevka-font-ttf@main/ttf/iosevka-regular.ttf', label: 'Iosevka Regular' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/nicholasgasior/iosevka-font-ttf@main/ttf/iosevka-bold.ttf', label: 'Iosevka Bold' },
  ],
  victor: [
    { type: 'local', matchers: ['victor mono', 'victormono'], label: 'Victor Mono (Local)' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/rubjo/victor-mono@v1.5.6/public/VictorMono-Regular.ttf', label: 'Victor Mono Regular' },
    { type: 'url', url: 'https://cdn.jsdelivr.net/gh/rubjo/victor-mono@v1.5.6/public/VictorMono-Bold.ttf', label: 'Victor Mono Bold' },
  ],
};

/**
 * Build fontSources array based on restty_font setting.
 * @param {string|null} resttyFont - Setting value (null = JetBrains Mono default)
 * @param {string} fontFamily - CSS fontFamily from Den settings (for local fallback)
 */
function buildFontSources(resttyFont, fontFamily) {
  const sources = [];
  // 1. Selected CDN font (if not default JetBrains Mono)
  if (resttyFont && CDN_FONT_MAP[resttyFont]) {
    sources.push(...CDN_FONT_MAP[resttyFont]);
  }
  // 2. User-configured CSS fonts from Den settings (local)
  if (fontFamily) {
    const families = fontFamily.split(',').map(f => f.trim().replace(/^["']|["']$/g, ''));
    for (const family of families) {
      const lower = family.toLowerCase();
      if (lower === 'monospace' || lower === 'sans-serif' || lower === 'serif') continue;
      sources.push({ type: 'local', matchers: [lower], label: family });
    }
  }
  // 3. Full restty fallback chain (JetBrains Mono CDN + symbols + emoji + CJK)
  sources.push(...RESTTY_FALLBACK_SOURCES);
  return sources;
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
    // Read restty_font setting (requires DenSettings global)
    const resttyFont = typeof DenSettings !== 'undefined' ? DenSettings.get('restty_font') : null;
    this._fontSources = buildFontSources(resttyFont, fontFamily);
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

    // Pass fontSources with user's preferred fonts + full restty fallback
    // chain (monospace CDN, symbols, emoji, CJK). This preserves Den's font
    // settings while keeping all the unicode coverage from restty's defaults.
    this._restty = new Restty({
      root: parent,
      fontSources: this._fontSources,
      appOptions: () => ({
        renderer: 'auto',
        autoResize: true,
        fontSize: this._fontSize,
        fontSizeMode: 'em',
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
