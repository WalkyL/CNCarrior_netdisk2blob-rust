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

const SOURCE_REVIEW_DAYS = 90;

function setText(id, value) {
  const node = document.getElementById(id);
  if (node) {
    node.textContent = value;
  }
}

function createNode(tagName, className, text) {
  const node = document.createElement(tagName);
  if (className) {
    node.className = className;
  }
  if (text) {
    node.textContent = text;
  }
  return node;
}

function renderCommand(label, command) {
  if (!command) {
    return null;
  }
  const wrapper = createNode('div', 'command-block');
  wrapper.appendChild(createNode('div', 'command-label', label));
  const pre = document.createElement('pre');
  const code = document.createElement('code');
  code.textContent = command;
  pre.appendChild(code);
  wrapper.appendChild(pre);
  return wrapper;
}

function renderPlatformCard(item) {
  const card = createNode('article', 'card platform-card');
  const title = createNode('h3', '', item.name || item.id);
  const meta = createNode('div', 'platform-meta');
  meta.appendChild(createNode('span', `status-pill status-${item.status || 'official'}`, item.status || 'official'));
  meta.appendChild(createNode('span', 'status-pill', item.arch || 'arch varies'));
  meta.appendChild(createNode('span', 'status-pill', item.service_mode || 'service mode'));

  const packageLine = createNode('p');
  packageLine.textContent = '发布物：';
  const packageCode = createNode('code', '', item.package || 'release artifact');
  packageLine.appendChild(packageCode);

  card.appendChild(title);
  card.appendChild(meta);
  card.appendChild(packageLine);

  const command = renderCommand('install', item.command);
  if (command) {
    card.appendChild(command);
  }
  const fallback = renderCommand('fallback', item.fallback_command);
  if (fallback) {
    card.appendChild(fallback);
  }
  const verify = renderCommand('verify', item.verify);
  if (verify) {
    card.appendChild(verify);
  }
  return card;
}

function renderCatalog(root, catalog) {
  root.replaceChildren();
  (catalog.groups || []).forEach((group) => {
    const section = createNode('section', 'catalog-group');
    const header = createNode('div', 'catalog-group-header');
    const heading = createNode('h3', '', group.title || group.id);
    const summary = createNode('p', 'section-lead', group.summary || '');
    header.appendChild(heading);
    header.appendChild(summary);
    section.appendChild(header);

    const grid = createNode('div', 'platform-grid');
    (group.items || []).forEach((item) => {
      grid.appendChild(renderPlatformCard(item));
    });
    section.appendChild(grid);
    root.appendChild(section);
  });
}

function renderSummary(root, catalog) {
  const official = (catalog.groups || []).find((group) => group.id === 'official-hosts');
  if (!official) {
    return;
  }
  root.replaceChildren();
  official.items.slice(0, 6).forEach((item) => {
    const card = createNode('article', 'card');
    card.appendChild(createNode('h3', '', item.name));
    card.appendChild(createNode('p', '', `${item.arch} · ${item.service_mode}`));
    card.appendChild(createNode('code', '', item.command));
    root.appendChild(card);
  });
}

async function loadInstallCatalog() {
  const catalogRoot = document.querySelector('[data-install-catalog]');
  const summaryRoot = document.querySelector('[data-install-summary]');
  if (!catalogRoot && !summaryRoot) {
    return;
  }
  try {
    const response = await fetch('/data/install-catalog.json', { cache: 'no-store' });
    if (!response.ok) {
      throw new Error(`install catalog HTTP ${response.status}`);
    }
    const catalog = await response.json();
    if (catalogRoot) {
      renderCatalog(catalogRoot, catalog);
    }
    if (summaryRoot) {
      renderSummary(summaryRoot, catalog);
    }
  } catch (error) {
    console.warn('Failed to load CCBG install catalog', error);
  }
}

function renderStaticFields() {
  setText('source-review-days', String(SOURCE_REVIEW_DAYS));
  setText('footer-fingerprint', CCBG_PROVENANCE.release_fingerprint);
}

window.CCBG_PROVENANCE = CCBG_PROVENANCE;
renderStaticFields();
loadInstallCatalog();

//# sourceMappingURL=app.js.map
