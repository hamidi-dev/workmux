import { describe, expect, test } from 'bun:test';

import { WorkmuxStatusPlugin } from '../resources/opencode/plugins/workmux-status';

async function createHarness() {
  const statuses: string[] = [];
  const commands: string[] = [];
  const shell = (strings: TemplateStringsArray, ...values: string[]) => ({
    quiet: async () => {
      statuses.push(values[0]);
      commands.push(
        strings.reduce((acc, part, i) => acc + part + (values[i] ?? ''), '').trim(),
      );
    },
  });
  const hooks = await WorkmuxStatusPlugin({ $: shell } as never);

  return {
    statuses,
    commands,
    emit: async (event: unknown) => {
      await hooks.event?.({ event } as never);
    },
  };
}

const sessionStatus = (sessionID: string, type: 'busy' | 'idle') => ({
  type: 'session.status',
  properties: { sessionID, status: { type } },
});

const userMessage = (sessionID: string) => ({
  type: 'message.updated',
  properties: { sessionID, info: { role: 'user', sessionID } },
});

const sessionCreated = (id: string, parentID?: string) => ({
  type: 'session.created',
  properties: { info: { id, ...(parentID ? { parentID } : {}) } },
});

describe('WorkmuxStatusPlugin', () => {
  test('stays working when a child session finishes before its parent', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit(sessionStatus('child', 'busy'));
    await harness.emit(sessionStatus('child', 'idle'));

    expect(harness.statuses).toEqual(['working']);

    await harness.emit(sessionStatus('parent', 'idle'));
    expect(harness.statuses).toEqual(['working', 'done']);
  });

  test('stays working when a parent session idles before its child', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit(sessionStatus('child', 'busy'));
    await harness.emit(sessionStatus('parent', 'idle'));

    expect(harness.statuses).toEqual(['working']);

    await harness.emit(sessionStatus('child', 'idle'));
    expect(harness.statuses).toEqual(['working', 'done']);
  });

  test('forgets an active session when OpenCode deletes it', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit(sessionStatus('child', 'busy'));
    await harness.emit(sessionStatus('parent', 'idle'));
    await harness.emit({
      type: 'session.deleted',
      properties: { info: { id: 'child' } },
    });
    await harness.emit(sessionStatus('child', 'busy'));

    expect(harness.statuses).toEqual(['working', 'done']);
  });

  test('ignores deletion of an untracked session', async () => {
    const harness = await createHarness();

    await harness.emit({
      type: 'session.deleted',
      properties: { info: { id: 'historical' } },
    });

    expect(harness.statuses).toEqual([]);
  });

  test('ignores idle status from an untracked session', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'idle'));

    expect(harness.statuses).toEqual([]);
  });

  test('ignores stale busy events until a new user message', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit(sessionStatus('parent', 'idle'));
    await harness.emit(sessionStatus('parent', 'busy'));
    expect(harness.statuses).toEqual(['working', 'done']);

    await harness.emit(userMessage('parent'));
    await harness.emit(sessionStatus('parent', 'busy'));
    expect(harness.statuses).toEqual(['working', 'done', 'working']);
  });

  test('reports the root session id and never a subagent one', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit(sessionStatus('child', 'busy'));
    await harness.emit(sessionStatus('child', 'idle'));
    await harness.emit(sessionStatus('parent', 'idle'));

    expect(harness.commands).toEqual([
      'workmux set-window-status working --session-id parent',
      'workmux set-window-status done --session-id parent',
    ]);
  });

  test('adopts a new root session after the old one is deleted', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('first', 'busy'));
    await harness.emit(sessionStatus('first', 'idle'));
    await harness.emit({
      type: 'session.deleted',
      properties: { info: { id: 'first' } },
    });
    await harness.emit(sessionStatus('second', 'busy'));

    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status working --session-id second',
    );
  });

  test('a surviving child never inherits the deleted root session', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit(sessionStatus('child', 'busy'));
    await harness.emit({
      type: 'session.deleted',
      properties: { info: { id: 'parent' } },
    });
    await harness.emit(sessionStatus('child', 'idle'));

    expect(harness.commands.some((c) => c.includes('--session-id child'))).toBe(
      false,
    );

    // A session started after the deletion is a new conversation, not a
    // descendant, so it does take the free slot.
    await harness.emit(sessionStatus('successor', 'busy'));
    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status working --session-id successor',
    );
  });

  test('follows a new session started next to the old one', async () => {
    const harness = await createHarness();

    // `/new` does not delete the session it replaces, so the pane has to
    // follow the created one or it reports the old conversation forever.
    await harness.emit(sessionStatus('first', 'busy'));
    await harness.emit(sessionStatus('first', 'idle'));
    await harness.emit(sessionCreated('second'));

    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status done --session-id second',
    );

    await harness.emit(userMessage('second'));
    await harness.emit(sessionStatus('second', 'busy'));
    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status working --session-id second',
    );
  });

  test('a trailing event from the replaced session does not take the pane back', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('first', 'busy'));
    await harness.emit(sessionCreated('second'));
    await harness.emit(sessionStatus('first', 'idle'));

    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status done --session-id second',
    );
  });

  test('a subagent session never takes the pane, not even on its own message', async () => {
    const harness = await createHarness();

    await harness.emit(sessionCreated('parent'));
    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit(sessionCreated('child', 'parent'));
    await harness.emit(userMessage('child'));
    await harness.emit(sessionStatus('child', 'busy'));
    await harness.emit(sessionStatus('child', 'idle'));
    await harness.emit(sessionStatus('parent', 'idle'));

    expect(harness.commands.every((c) => !c.includes('--session-id child'))).toBe(
      true,
    );
    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status done --session-id parent',
    );
  });

  test('follows the session the user switches back to', async () => {
    const harness = await createHarness();

    await harness.emit(sessionCreated('first'));
    await harness.emit(sessionStatus('first', 'busy'));
    await harness.emit(sessionStatus('first', 'idle'));
    await harness.emit(sessionCreated('second'));
    await harness.emit(sessionStatus('second', 'busy'));
    await harness.emit(sessionStatus('second', 'idle'));

    // Switching sessions creates nothing -- the next prompt is the signal.
    await harness.emit(userMessage('first'));
    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status done --session-id first',
    );

    await harness.emit(sessionStatus('first', 'busy'));
    expect(harness.commands.at(-1)).toBe(
      'workmux set-window-status working --session-id first',
    );
  });

  test('reports waiting while another session is working', async () => {
    const harness = await createHarness();

    await harness.emit(sessionStatus('parent', 'busy'));
    await harness.emit({
      type: 'question.asked',
      properties: { sessionID: 'child' },
    });
    await harness.emit(sessionStatus('parent', 'idle'));
    expect(harness.statuses).toEqual(['working', 'waiting']);

    await harness.emit({
      type: 'question.replied',
      properties: { sessionID: 'child' },
    });
    expect(harness.statuses).toEqual(['working', 'waiting', 'working']);
  });
});
