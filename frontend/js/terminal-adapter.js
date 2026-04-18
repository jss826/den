// Terminal Adapter — selects xterm.js, restty, or wterm based on settings
// Must be loaded after xterm vendor scripts and DenSettings, before terminal.js
/* global Terminal, FitAddon, WebglAddon, DenSettings, Toast */
const TerminalAdapter = (() => {
  let _ready = null;

  /**
   * Initialize the adapter with the chosen renderer.
   * @param {'xterm'|'restty'|'wterm'} renderer
   */
  function init(renderer) {
    if (renderer === 'restty') {
      _ready = loadRestty();
    } else if (renderer === 'wterm') {
      _ready = loadWterm();
    } else {
      _ready = Promise.resolve({
        TerminalClass: Terminal,
        FitAddonClass: FitAddon.FitAddon,
        needsWebgl: true,
        isRestty: false,
        isWterm: false,
      });
    }
  }

  async function loadRestty() {
    try {
      const { DenResttyTerminal, NoopFitAddon } = await import('/vendor/restty/restty-xterm-adapter.js?v=12');
      return {
        TerminalClass: DenResttyTerminal,
        FitAddonClass: NoopFitAddon,
        needsWebgl: false,
        isRestty: true,
        isWterm: false,
      };
    } catch (e) {
      console.error('[TerminalAdapter] Failed to load restty, falling back to xterm.js:', e);
      if (typeof Toast !== 'undefined') {
        Toast.error('restty failed to load, using xterm.js');
      }
      return {
        TerminalClass: Terminal,
        FitAddonClass: FitAddon.FitAddon,
        needsWebgl: true,
        isRestty: false,
        isWterm: false,
      };
    }
  }

  async function loadWterm() {
    try {
      const { DenWtermTerminal, NoopFitAddon } = await import('/vendor/wterm/wterm-xterm-adapter.js?v=14');
      return {
        TerminalClass: DenWtermTerminal,
        FitAddonClass: NoopFitAddon,
        needsWebgl: false,
        isRestty: false,
        isWterm: true,
      };
    } catch (e) {
      console.error('[TerminalAdapter] Failed to load wterm, falling back to xterm.js:', e);
      if (typeof Toast !== 'undefined') {
        Toast.error('wterm failed to load, using xterm.js');
      }
      return {
        TerminalClass: Terminal,
        FitAddonClass: FitAddon.FitAddon,
        needsWebgl: true,
        isRestty: false,
        isWterm: false,
      };
    }
  }

  /**
   * Returns a Promise that resolves to the terminal classes.
   * @returns {Promise<{TerminalClass, FitAddonClass, needsWebgl: boolean, isRestty: boolean, isWterm: boolean}>}
   */
  function ready() {
    if (!_ready) {
      init('xterm');
    }
    return _ready;
  }

  return { init, ready };
})();
