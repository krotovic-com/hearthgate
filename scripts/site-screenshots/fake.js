// Example upstreams and a stand-in for an OpenAI-compatible model with fixed answers,
// for scripts/site-screenshots/run.sh.
const http = require('http');
for (const p of [8081, 8082, 8083, 8084, 2283]) http.createServer((q, r) => r.end('ok')).listen(p, '127.0.0.1');
require('net').createServer((s) => s.end('SSH-2.0-OpenSSH_9.9\r\n')).listen(8022, '127.0.0.1');
const svc = { name: 'immich', kind: 'external', host: '127.0.0.1', ports: { http: 2283 }, health: { type: 'http', path: '/api/server/ping', interval: '10s' } };
const steps = (n) => [
  { title: 'Add Immich as a service', detail: 'Port 2283 on this machine, with a health check.' },
  { title: 'Add photos.example.com to the web interface', detail: 'Your *.example.com certificate already covers it.' },
  { title: 'Route photos.example.com to Immich', detail: 'HTTPS on 443, HTTP redirects.' },
  { title: 'Check it from outside', detail: 'Open the address on your phone with Wi-Fi off.' },
].map((s, i) => ({ ...s, done: i < n }));
const plan = (n) => ({ title: 'Publish Immich at photos.example.com', steps: steps(n) });
function chat(body) {
  const users = (body.match(/\[user\]/g) || []).length;
  if (users <= 1) return {
    reply: "Here's the plan. It takes four small changes, and I'll propose them one at a time. First, add Immich as a service so routes can reach it. I added a health check on Immich's ping endpoint, so the dashboard shows when it is down.",
    proposals: [{ kind: 'service', title: 'Add the immich service', summary: 'External service on this machine, port 2283, health check every 10 s.', step: 1, changes: [{ op: 'add', path: '/services/-', value_json: JSON.stringify(svc) }] }],
    plan: plan(0),
    suggestions: ['Can I keep it VPN-only?', 'Why a health check?'],
  };
  return {
    reply: 'Immich is up and its health check passes. Next, add photos.example.com to the web interface. The certificate you uploaded covers *.example.com, so no new certificate is needed and nothing new opens in the firewall: 443 is already public.',
    proposals: [{ kind: 'interface', title: 'Add photos.example.com to web', summary: 'One more domain on the web interface. Ports stay 80 and 443.', step: 2, changes: [{ op: 'add', path: '/interfaces/0/domains/-', value_json: JSON.stringify('photos.example.com') }] }],
    plan: plan(1),
    suggestions: ['Limit uploads to the VPN?', 'What does the route do?'],
  };
}
function review() {
  return {
    summary: 'Your setup is in good shape. Three things to look at, one of them important.',
    findings: [
      { severity: 'high', area: 'firewall', title: 'The firewall is off', detail: 'Every port a program listens on is reachable, not only the ones your public interfaces open. Turning it on closes the rest; a change that locks you out undoes itself after 60 seconds.', fix: { summary: 'Turn the firewall on', changes: [{ op: 'add', path: '/firewall/enabled', value_json: 'true' }] } },
      { severity: 'medium', area: 'interfaces', title: 'SSH is published on port 2222', detail: 'The ssh interface makes sshd reachable from the internet. Over the VPN you reach it at sshd.home.internal without a public port.' },
      { severity: 'low', area: 'interfaces', title: 'No rate limit on the web interface', detail: 'A per-client limit of new connections per second slows down scanners and brute-force attempts without affecting normal use.', fix: { summary: 'Limit new connections to 30 per second per client', changes: [{ op: 'add', path: '/interfaces/0/rate_limit', value_json: '30' }] } },
    ],
  };
}
http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', () => {
    const answer = body.includes('assistant inside the admin dashboard') ? chat(body) : review();
    setTimeout(() => {
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify({ choices: [{ message: { content: JSON.stringify(answer) } }] }));
    }, 300);
  });
}).listen(9900, '127.0.0.1');
