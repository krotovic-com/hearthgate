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

  test('every page passes WCAG AA checks', async ({ page }) => {
    await login(page);
    for (const [link, heading] of [
      ['Overview', 'Overview'],
      ['Public interfaces', 'Public interfaces'],
      ['Services', 'Services'],
      ['Routes', 'Routes'],
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
