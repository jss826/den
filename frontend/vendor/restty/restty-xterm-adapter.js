// restty xterm.js adapter for Den
// Extends restty's xterm compat Terminal with stub methods that Den requires
// but restty does not implement (OSC handlers, title change, selection, etc.)

import { Terminal as ResttyTerminal } from './xterm.js';

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
 * DenTerminal adapter — wraps restty's xterm compat Terminal with
 * additional stubs for APIs Den uses but restty doesn't provide.
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

  constructor(options = {}) {
    // Extract theme before passing to restty (restty doesn't understand xterm theme objects)
    const { theme, ...restOptions } = options;
    super({
      ...restOptions,
      appOptions: {
        renderer: 'auto',
        autoResize: true,
        fontSize: options.fontSize || 15,
        touchSelectionMode: 'long-press',
      },
    });
    if (theme) {
      this._pendingTheme = theme;
    }
  }

  open(parent) {
    super.open(parent);
    // Apply theme after restty instance is created
    if (this._pendingTheme && this.restty) {
      const ghosttyTheme = xtermThemeToGhostty(this._pendingTheme);
      if (ghosttyTheme) {
        try {
          this.restty.applyTheme(ghosttyTheme, 'inline');
        } catch (e) {
          console.warn('[restty] Failed to apply theme:', e);
        }
      }
      this._pendingTheme = null;
    }
  }

  /**
   * Override write to handle Uint8Array (Den sends binary data from WebSocket).
   * restty's sendInput expects string, so decode if needed.
   */
  write(data, callback) {
    if (data instanceof Uint8Array || data instanceof ArrayBuffer) {
      const str = textDecoder.decode(data instanceof ArrayBuffer ? new Uint8Array(data) : data);
      super.write(str, callback);
    } else {
      super.write(data, callback);
    }
  }

  /** Override options setter to intercept theme changes */
  get options() {
    return super.options;
  }

  set options(next) {
    if (next && next.theme && this.restty) {
      const ghosttyTheme = xtermThemeToGhostty(next.theme);
      if (ghosttyTheme) {
        try {
          this.restty.applyTheme(ghosttyTheme, 'inline');
        } catch (e) {
          console.warn('[restty] Failed to apply theme:', e);
        }
      }
    }
    if (next && next.fontSize && this.restty) {
      try {
        this.restty.setFontSize(next.fontSize);
      } catch (_) { /* may not be supported */ }
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
  fit() {}
  proposeDimensions() { return null; }
}

export { DenResttyTerminal, NoopFitAddon, xtermThemeToGhostty };
