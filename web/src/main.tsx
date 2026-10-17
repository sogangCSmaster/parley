import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import '../../design/tokens.css'
import './index.css'
import App from './App.tsx'
import './lib/theme'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
