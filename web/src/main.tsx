import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { resolveBackend } from './api/bootstrap';
import './styles.css';

const container = document.getElementById('root');
if (!container) throw new Error('#root element missing');
const root = createRoot(container);

resolveBackend()
  .then((backend) => {
    root.render(
      <StrictMode>
        <App backend={backend} />
      </StrictMode>,
    );
  })
  .catch((error: unknown) => {
    root.render(
      <main className="fatal panel">
        <h1>PromptCAD Studio</h1>
        <p className="status error">
          無法連線到本機後端：{error instanceof Error ? error.message : String(error)}
        </p>
      </main>,
    );
  });
