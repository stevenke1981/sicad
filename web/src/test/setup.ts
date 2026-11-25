import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';

afterEach(() => {
  cleanup();
});

// jsdom has no WebGL and logs "not implemented" for getContext; report "unavailable" quietly.
Object.defineProperty(HTMLCanvasElement.prototype, 'getContext', {
  configurable: true,
  writable: true,
  value: vi.fn(() => null),
});

// jsdom lacks object URLs; the UI only needs stable strings for <img src>.
let objectUrlCounter = 0;
if (typeof URL.createObjectURL !== 'function') {
  Object.defineProperty(URL, 'createObjectURL', {
    configurable: true,
    writable: true,
    value: () => `blob:mock/${++objectUrlCounter}`,
  });
}
if (typeof URL.revokeObjectURL !== 'function') {
  Object.defineProperty(URL, 'revokeObjectURL', {
    configurable: true,
    writable: true,
    value: () => undefined,
  });
}

// jsdom logs "not implemented" for scrolling; the UI only scrolls for convenience.
Object.defineProperty(window, 'scrollTo', { configurable: true, writable: true, value: () => {} });
