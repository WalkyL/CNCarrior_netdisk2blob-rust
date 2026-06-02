// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky
const CCBG_PROVENANCE = {
  service: 'carrier-cloud-blob-gateway-public',
  version: '0.1.2',
  release_channel: 'public-materials',
  release_date: '2026-06-03',
  release_fingerprint: 'ccbg-0.1.2-walky-20260603-bda37a712441fe32',
  fingerprint_sha256: '03bae8a844f520528d9c677ca8c43773fc3b0a27d03ec7f9a32abf5cf57c258d',
  canonical_repo: 'https://github.com/WalkyL/CNCarrior_netdisk2blob-rust',
  license_id: 'LicenseRef-CCBG-Public-Materials'
};

const SOURCE_REVIEW_DAYS = 90;
const COPY_RESET_DELAY_MS = 1600;

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

async function copyCommandText(text) {
  const value = String(text || '');
  if (!value) {
    return false;
  }
  if (navigator.clipboard && window.isSecureContext) {
    await navigator.clipboard.writeText(value);
    return true;
  }
  const textarea = document.createElement('textarea');
  textarea.value = value;
  textarea.setAttribute('readonly', 'readonly');
  textarea.style.position = 'fixed';
  textarea.style.top = '-9999px';
  document.body.appendChild(textarea);
  textarea.select();
  const copied = document.execCommand('copy');
  document.body.removeChild(textarea);
  return copied;
}

function attachCopyButton(pre, command) {
  if (!pre || pre.dataset.copyReady === 'true') {
    return;
  }
  pre.dataset.copyReady = 'true';
  pre.classList.add('command-pre');
  const button = createNode('button', 'command-copy-button', 'Copy');
  button.type = 'button';
  button.addEventListener('click', async () => {
    const original = button.textContent || 'Copy';
    button.disabled = true;
    try {
      const copied = await copyCommandText(command);
      button.textContent = copied ? 'Copied' : 'Copy failed';
    } catch (_error) {
      button.textContent = 'Copy failed';
    }
    window.setTimeout(() => {
      button.textContent = original;
      button.disabled = false;
    }, COPY_RESET_DELAY_MS);
  });
  pre.appendChild(button);
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
  attachCopyButton(pre, command);
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
  if (item.upgrade_note) {
    card.appendChild(createNode('p', 'section-lead', item.upgrade_note));
  }

  const command = renderCommand('install', item.command);
  if (command) {
    card.appendChild(command);
  }
  const upgrade = renderCommand('upgrade', item.upgrade);
  if (upgrade) {
    card.appendChild(upgrade);
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

function enhanceStaticInstallCommands() {
  document.querySelectorAll('.install-console pre').forEach((pre) => {
    const code = pre.querySelector('code');
    if (!code) {
      return;
    }
    attachCopyButton(pre, code.textContent || '');
  });
}

function renderStaticFields() {
  setText('source-review-days', String(SOURCE_REVIEW_DAYS));
  setText('footer-fingerprint', CCBG_PROVENANCE.release_fingerprint);
}

window.CCBG_PROVENANCE = CCBG_PROVENANCE;
renderStaticFields();
enhanceStaticInstallCommands();
loadInstallCatalog();

//# sourceMappingURL=app.js.map
