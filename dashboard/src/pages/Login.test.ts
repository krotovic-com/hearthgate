import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Login from './Login.svelte';
import { session } from '../lib/session.svelte';

function reply(status: number, body: unknown) {
  return Promise.resolve(new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } }));
}

describe('Login', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('asks for the authenticator code when the password was right', async () => {
    const fetch = vi
      .fn()
      .mockReturnValueOnce(reply(401, { error: 'enter the code from your authenticator app', totp_required: true }))
      .mockReturnValueOnce(
        reply(200, { setup_required: false, user: { username: 'admin', totp_enabled: true }, csrf: 'c' }),
      );
    vi.stubGlobal('fetch', fetch);
    render(Login);
    await fireEvent.input(screen.getByLabelText('Username'), { target: { value: 'admin' } });
    await fireEvent.input(screen.getByLabelText('Password'), { target: { value: 'correct horse battery' } });
    await fireEvent.click(screen.getByRole('button', { name: /log in/i }));
    const code = await screen.findByLabelText('Authenticator code');
    await fireEvent.input(code, { target: { value: '123456' } });
    await fireEvent.click(screen.getByRole('button', { name: /log in|continue|verify/i }));
    await vi.waitFor(() => expect(session.info?.user?.username).toBe('admin'));
    const body = JSON.parse(fetch.mock.calls[1]![1].body as string);
    expect(body).toEqual({ username: 'admin', password: 'correct horse battery', code: '123456' });
  });

  it('shows the server message for a wrong password', async () => {
    vi.stubGlobal('fetch', vi.fn().mockReturnValue(reply(401, { error: 'wrong username or password', totp_required: false })));
    render(Login);
    await fireEvent.input(screen.getByLabelText('Username'), { target: { value: 'admin' } });
    await fireEvent.input(screen.getByLabelText('Password'), { target: { value: 'nope nope nope' } });
    await fireEvent.click(screen.getByRole('button', { name: /log in/i }));
    expect(await screen.findByRole('alert')).toHaveTextContent('wrong username or password');
  });
});
