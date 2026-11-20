import { execFileSync } from 'node:child_process';
import { request as httpRequest } from 'node:http';
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { server } from './server';

const PASSWORD = 'correct horse battery staple';
const DOMAIN = 'app.example.test';

/** A request through the gateway's public port, as a visitor would send it. */
function visit(path = '/'): Promise<{ status: number; body: string }> {
  const { publicPort } = server();
  return new Promise((ok, fail) => {
    const req = httpRequest({ host: '127.0.0.1', port: publicPort, path, headers: { host: DOMAIN }, agent: false }, (res) => {
      let body = '';
      res.on('data', (c) => (body += c));
      res.on('end', () => ok({ status: res.statusCode ?? 0, body }));
    });
    req.on('error', fail);
    req.end();
  });
}

async function eventually(check: () => Promise<boolean>, what: string): Promise<void> {
  for (let i = 0; i < 50; i++) {
    if (await check().catch(() => false)) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`timed out waiting for ${what}`);
}

async function accessible(page: Page): Promise<void> {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(results.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

async function login(page: Page): Promise<void> {
  await page.goto(server().baseURL);
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill(PASSWORD);
  await page.getByRole('button', { name: 'Log in' }).click();
  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
}

async function applyDraft(page: Page, expectInDiff: string): Promise<void> {
  await page.getByRole('button', { name: 'Review and apply' }).click();
  const dialog = page.getByRole('dialog', { name: 'Review changes' });
  await expect(dialog.getByLabel('Changes', { exact: true })).toContainText(expectInDiff);
  await dialog.getByRole('button', { name: 'Apply changes' }).click();
  await expect(page.getByText(/Applied as revision \d+/)).toBeVisible();
}

test.describe.serial('dashboard', () => {
  test('first-run setup creates the admin account', async ({ page }) => {
    const { bin, config, baseURL } = server();
    const code = execFileSync(bin, ['-c', config, 'setup-code']).toString().trim();
    await page.goto(baseURL);
    await expect(page.getByRole('heading', { name: 'Create the admin account' })).toBeVisible();
    await accessible(page);
    await page.getByLabel('Setup code').fill(code);
    await page.getByLabel('Username').fill('admin');
    await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
    await page.getByLabel('Repeat password').fill(PASSWORD);
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
    await expect(page.getByText('Publish your first service')).toBeVisible();
  });

  test('publishes a service without touching a config file', async ({ page }) => {
    const { publicPort, upstreamPort } = server();
    await login(page);

    await page.getByRole('button', { name: 'Add a service' }).click();
    const svc = page.getByRole('dialog', { name: 'Add a service' });
    await svc.getByLabel('Host').fill('127.0.0.1');
    await svc.getByLabel('Name', { exact: true }).fill('app');
    await svc.getByLabel('Port', { exact: true }).fill(String(upstreamPort));
    await svc.getByRole('button', { name: 'Add to draft' }).click();

    await page.getByRole('link', { name: 'Public interfaces', exact: true }).click();
    await page.getByRole('button', { name: 'Add public interface' }).click();
    const iface = page.getByRole('dialog', { name: 'Add a public interface' });
    await iface.getByLabel('Domains').fill(DOMAIN);
    await expect(iface.getByLabel('Name', { exact: true })).toHaveValue('app-example-test');
    // A plain HTTP port keeps the test away from Let's Encrypt.
    await iface.getByRole('button', { name: 'Remove port 80' }).click();
    await iface.getByLabel('Port', { exact: true }).fill(String(publicPort));
    await iface.getByLabel('Handles').selectOption('http');
    await iface.getByRole('button', { name: 'Add to draft' }).click();

    await page.getByRole('link', { name: 'Routes', exact: true }).click();
    await page.getByRole('button', { name: 'New route' }).click();
    await page.getByRole('button', { name: 'Save to draft' }).click();
    await expect(page.getByText('3 changes not applied yet')).toBeVisible();

    await applyDraft(page, `+domains = ["${DOMAIN}"]`);
    await eventually(async () => (await visit('/hi')).body === 'hello from upstream /hi', 'the route to serve');

    await page.getByRole('link', { name: 'Services', exact: true }).click();
    await page.getByRole('link', { name: 'app', exact: true }).click();
    await expect(page.getByRole('complementary').getByText('Running')).toBeVisible();
  });

  test('history shows diffs and rolls back', async ({ page }) => {
    await login(page);
    await page.getByRole('link', { name: 'Routes', exact: true }).click();
    await page.getByRole('link', { name: /app-example-test/ }).click();
    await page.getByRole('button', { name: 'Delete' }).click();
    await applyDraft(page, '-[[routes]]');
    // The port answers 404 or is closed, depending on whether anything else uses it.
    await eventually(() => visit().then((r) => r.status !== 200, () => true), 'the route to go away');

    await page.getByRole('link', { name: 'History', exact: true }).click();
    await page.getByRole('link', { name: /#1/ }).click();
    await expect(page.getByLabel('Changes', { exact: true })).toContainText('+[[routes]]');
    await page.getByRole('button', { name: 'Roll back to this' }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Roll back' }).click();
    await expect(page.getByText(/Revision 1 is running again/)).toBeVisible();
    await eventually(async () => (await visit()).status === 200, 'the rollback to apply');
  });

  test('firewall rules and blocklists go through the draft', async ({ page }) => {
    await login(page);
    await page.getByRole('link', { name: 'Firewall', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'The firewall is off' })).toBeVisible();

    await page.getByRole('button', { name: 'Add rule' }).click();
    const rule = page.getByRole('dialog', { name: 'Add a firewall rule' });
    await rule.getByLabel('Name').fill('ssh-from-office');
    await rule.getByLabel('From').fill('192.0.2.0/24, 198.51.100.7');
    await rule.getByLabel('Ports').fill('22');
    await rule.getByRole('button', { name: 'Add to draft' }).click();
    await expect(page.getByText('From 192.0.2.0/24, 198.51.100.7 to 22/tcp')).toBeVisible();

    await page.getByRole('button', { name: 'Blocklists', exact: true }).click();
    const lists = page.getByRole('dialog', { name: 'Blocklists' });
    await lists.getByRole('button', { name: 'Add firehol-level1' }).click();
    await lists.getByLabel('Countries').fill('xx, YY');
    await lists.getByRole('button', { name: 'Save to draft' }).click();
    await expect(page.getByText('firehol-level1 · countries XX, YY')).toBeVisible();

    await applyDraft(page, 'ssh-from-office');
    // Stored as the server normalizes it, and the draft is clean again.
    await expect(page.getByRole('button', { name: 'Review and apply' })).toBeHidden();
    await expect(page.getByText('From 192.0.2.0/24, 198.51.100.7 to 22/tcp')).toBeVisible();

    // Router port forwarding is a draft change too.
    await page.getByRole('link', { name: 'Settings', exact: true }).click();
    await page.getByRole('switch', { name: 'Forward ports automatically' }).click();
    await expect(page.getByText('Turn on router port forwarding')).toBeVisible();
    await page.getByRole('button', { name: 'Discard' }).click();
  });

  test('adds a VPN device and shows its configuration once', async ({ page }) => {
    await login(page);
    await page.getByRole('link', { name: 'VPN peers', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'The VPN is off' })).toBeVisible();

    // Devices connect to the public domain unless told otherwise.
    await expect(page.getByText('Now app.example.test:51820.')).toBeVisible();
    await page.getByLabel('Device name').fill('Kitchen tablet');
    await expect(page.getByText('Gets address 10.88.0.2 and the name kitchen-tablet.home.internal.')).toBeVisible();
    await page.getByLabel('Devices connect to').fill('vpn.example.test');
    await page.getByLabel('Devices connect to').press('Tab');
    await page.getByText('Only some services').click();
    await page.getByRole('checkbox', { name: 'app' }).check();
    await accessible(page);
    await page.getByRole('button', { name: 'Create device' }).click();

    const card = page.locator('#add-device');
    await expect(card.getByRole('img', { name: 'WireGuard configuration for kitchen-tablet' })).toBeVisible();
    await expect(card.getByText('kitchen-tablet gets 10.88.0.2. Apply the draft so it can connect.')).toBeVisible();
    const download = page.waitForEvent('download');
    await card.getByRole('link', { name: 'Download .conf' }).click();
    const file = await download;
    expect(file.suggestedFilename()).toBe('kitchen-tablet.conf');
    const conf = await new Promise<string>((ok, fail) => {
      file.createReadStream().then((stream) => {
        let text = '';
        stream.on('data', (c) => (text += c));
        stream.on('end', () => ok(text));
        stream.on('error', fail);
      }, fail);
    });
    expect(conf).toContain('Endpoint = vpn.example.test:51820');
    expect(conf).toContain('DNS = 10.88.0.1, home.internal');
    expect(conf).toMatch(/PrivateKey = [A-Za-z0-9+/]{43}=/);
    await accessible(page);
    await page.screenshot({ path: 'test-results/vpn-peers.png', fullPage: true });

    // The device is in the draft, with the VPN turned on.
    const row = page.getByRole('listitem').filter({ hasText: 'kitchen-tablet.home.internal' });
    await expect(row.getByText('Not applied')).toBeVisible();
    await expect(row.getByText('Only app')).toBeVisible();
    await expect(page.getByText('Add VPN device kitchen-tablet')).toBeVisible();
    await expect(page.getByText('Turn on the VPN')).toBeVisible();

    await row.getByRole('button', { name: 'Edit kitchen-tablet' }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit kitchen-tablet' });
    await dialog.getByText('Everything').click();
    await accessible(page);
    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(row.getByText('Everything')).toBeVisible();

    // Applying would start a real WireGuard interface, so leave it here.
    await page.getByRole('button', { name: 'Discard' }).click();
    await expect(page.getByText('No devices yet')).toBeVisible();
  });

  // Installs a real nftables ruleset, so it only runs when asked for (as
  // root, in a throwaway machine or container).
  test('a firewall change is kept once the dashboard confirms it', async ({ page }) => {
    test.skip(process.env.HEARTHGATE_E2E_FIREWALL !== '1', 'set HEARTHGATE_E2E_FIREWALL=1 to run');
    await login(page);
    await page.getByRole('link', { name: 'Firewall', exact: true }).click();
    await page.getByRole('button', { name: 'Turn on the firewall' }).click();
    await applyDraft(page, 'enabled = true');
    await expect(page.getByText('Firewall change confirmed')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText('Deny all')).toBeVisible();
    await page.screenshot({ path: 'test-results/firewall-on.png', fullPage: true });
    // Still reachable and still on after the confirmation window.
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Firewall', level: 1 })).toBeVisible();
    await page.getByRole('switch', { name: 'Firewall' }).click();
    await applyDraft(page, 'enabled = false');
  });

  test('every page passes WCAG AA checks', async ({ page }) => {
    await login(page);
    for (const [link, heading] of [
      ['Overview', 'Overview'],
      ['Public interfaces', 'Public interfaces'],
      ['Services', 'Services'],
      ['Routes', 'Routes'],
      ['VPN peers', 'VPN peers'],
      ['Firewall', 'Firewall'],
      ['History', 'History'],
      ['Settings', 'Settings'],
    ]) {
      await page.getByRole('link', { name: link, exact: true }).click();
      await expect(page.getByRole('heading', { name: heading, level: 1 })).toBeVisible();
      await accessible(page);
    }
    await page.getByRole('link', { name: 'Public interfaces', exact: true }).click();
    await page.getByRole('button', { name: /^Edit/ }).first().click();
    await accessible(page);
    await page.keyboard.press('Escape');
    await page.getByRole('link', { name: 'Firewall', exact: true }).click();
    await page.getByRole('button', { name: 'Edit ssh-from-office' }).click();
    await accessible(page);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Blocklists', exact: true }).click();
    await accessible(page);
  });

  test('works with the keyboard alone', async ({ page }) => {
    await login(page);
    // Start from a fresh page load, as a keyboard user would.
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
    await page.keyboard.press('Tab');
    await expect(page.getByRole('link', { name: 'Skip to content' })).toBeFocused();
    // Through the logo to the navigation, then open Services with Enter.
    for (let i = 0; i < 4; i++) await page.keyboard.press('Tab');
    await expect(page.getByRole('link', { name: 'Services', exact: true })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: 'Services', level: 1 })).toBeFocused();
    // Dialogs keep focus inside and close with Escape.
    await page.getByRole('button', { name: 'Add service' }).focus();
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: 'Add a service' });
    await expect(dialog).toBeVisible();
    await expect(dialog.locator(':focus')).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
  });
});
