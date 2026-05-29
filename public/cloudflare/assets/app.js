// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky
const CCBG_PROVENANCE = {
  service: 'carrier-cloud-blob-gateway-public',
  version: '0.1.0',
  release_channel: 'public-materials',
  release_date: '2026-05-26',
  release_fingerprint: 'ccbg-0.1.0-walky-20260526-e756003d846d2c46',
  fingerprint_sha256: 'e756003d846d2c460f892a20402d59539c8c6980ba011c62d17ab5ad962de6b6',
  canonical_repo: 'https://github.com/walky/carrier-cloud-blob-gateway',
  license_id: 'LicenseRef-CCBG-Public-Materials'
};

const demoState = {
  primary: 'Unicom',
  sync: ['Telecom', 'Mobile'],
  queue: 7,
  failed: 0,
  sourceReviewDays: 90
};

function setText(id, value) {
  const node = document.getElementById(id);
  if (node) {
    node.textContent = value;
  }
}

function renderDemoState() {
  setText('demo-primary', demoState.primary);
  setText('demo-sync', demoState.sync.join(' / '));
  setText('demo-queue', String(demoState.queue));
  setText('demo-failed', String(demoState.failed));
  setText('source-review-days', String(demoState.sourceReviewDays));
  setText('footer-fingerprint', CCBG_PROVENANCE.release_fingerprint);
}

function rotatePrimary() {
  const providers = ['Unicom', 'Telecom', 'Mobile'];
  const currentIndex = providers.indexOf(demoState.primary);
  demoState.primary = providers[(currentIndex + 1) % providers.length];
  demoState.sync = providers.filter((provider) => provider !== demoState.primary);
  demoState.queue = Math.max(2, demoState.queue + 3);
  renderDemoState();
}

function acknowledgeQueue() {
  demoState.queue = Math.max(0, demoState.queue - 4);
  renderDemoState();
}

function wireDemoControls() {
  const rotate = document.getElementById('rotate-primary');
  const acknowledge = document.getElementById('acknowledge-queue');
  if (rotate) {
    rotate.addEventListener('click', rotatePrimary);
  }
  if (acknowledge) {
    acknowledge.addEventListener('click', acknowledgeQueue);
  }
}

window.CCBG_PROVENANCE = CCBG_PROVENANCE;
renderDemoState();
wireDemoControls();

//# sourceMappingURL=app.js.map
