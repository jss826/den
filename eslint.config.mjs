import js from '@eslint/js';
import globals from 'globals';

export default [
  js.configs.recommended,
  {
    files: ['frontend/js/**/*.js'],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'script',
      globals: {
        ...globals.browser,
        // App globals (IIFE modules)
        Auth: 'writable',
        DenTerminal: 'writable',
        Keybar: 'writable',
        DenMarkdown: 'writable',
        DenSettings: 'writable',
        Toast: 'readonly',
        Spinner: 'readonly',
        DenIcons: 'readonly',
        DenClipboard: 'readonly',
        ClipboardHistory: 'readonly',
        FloatTerminal: 'writable',
        DenSnippet: 'readonly',
        DenDragList: 'readonly',
        DenKeyPresets: 'readonly',
        DenFiler: 'readonly',
        FilerTree: 'readonly',
        FilerEditor: 'readonly',
        FilerRemote: 'readonly',
        CM: 'readonly',
        // xterm.js vendor globals
        Terminal: 'readonly',
        FitAddon: 'readonly',
        WebglAddon: 'readonly',
        // Node.js guard for module.exports
        module: 'readonly',
      },
    },
    rules: {
      // Script-mode IIFEs intentionally redefine globals
      'no-redeclare': ['error', { builtinGlobals: false }],
      'no-unused-vars': ['warn', {
        argsIgnorePattern: '^_',
        varsIgnorePattern: '^(Auth|DenTerminal|Keybar|DenMarkdown|DenSettings|FloatTerminal)$',
        caughtErrorsIgnorePattern: '^_',
      }],
      'no-console': 'off',
    },
  },
  {
    ignores: ['frontend/vendor/**'],
  },
];
