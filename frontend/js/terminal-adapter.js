// Terminal Adapter — selects xterm.js or restty based on settings
// Must be loaded after xterm vendor scripts and DenSettings, before terminal.js
/* global Terminal, FitAddon, WebglAddon, DenSettings, Toast */
const TerminalAdapter = (() => {
  let _ready = null;

  /**
   * Initialize the adapter with the chosen renderer.
   * @param {'xterm'|'restty'} renderer
   */
  function init(renderer) {
    if (renderer === 'restty') {
      _ready = loadRestty();
    } else {
      _ready = Promise.resolve({
        TerminalClass: Terminal,
        FitAddonClass: FitAddon.FitAddon,
        needsWebgl: true,
        isRestty: false,
      });
    }
  }

  async function loadRestty() {
    try {
      const { DenResttyTerminal, NoopFitAddon } = await import('/vendor/restty/restty-xterm-adapter.js?v=9');
      return {
        TerminalClass: DenResttyTerminal,
        FitAddonClass: NoopFitAddon,
        needsWebgl: false,
        isRestty: true,
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
      };
    }
  }

  /**
   * Returns a Promise that resolves to the terminal classes.
   * @returns {Promise<{TerminalClass, FitAddonClass, needsWebgl: boolean, isRestty: boolean}>}
   */
  function ready() {
    if (!_ready) {
      init('xterm');
    }
    return _ready;
  }

  return { init, ready };
})();
