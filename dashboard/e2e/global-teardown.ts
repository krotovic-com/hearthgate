import { server } from './server';

export default async function globalTeardown(): Promise<void> {
  for (const pid of server().pids) {
    try {
      process.kill(pid);
    } catch {
      // Already gone.
    }
  }
}
