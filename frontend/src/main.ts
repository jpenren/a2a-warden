/** Entry point of the embedded A2A Warden console. */

import { bootstrap, refreshStatus } from "./views";

bootstrap();

// Keep the health indicator fresh so a stopped warden is visible immediately.
window.setInterval(() => {
  void refreshStatus();
}, 5000);
