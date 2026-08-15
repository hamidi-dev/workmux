import type { Plugin } from '@opencode-ai/plugin';

export const WorkmuxStatusPlugin: Plugin = async ({ $ }) => {
  // OpenCode can emit repeated `session.status busy` events for a single turn,
  // and can even emit a stale trailing `busy` after `idle` at the end. Track
  // every parent and child session so one idle session cannot mark the whole
  // pane done while another session is still working.
  const statusBySession = new Map<string, string>();
  const acceptBusyBySession = new Map<string, boolean>();
  const deletedSessions = new Set<string>();
  let reportedStatus: string | undefined;
  // The pane's own session: the conversation the user is talking to. Workmux
  // records it against the pane, so subagent sessions must never displace it.
  let rootSession: string | undefined;
  let reportedSession: string | undefined;
  // Sessions OpenCode created underneath another session -- subagent work,
  // never the pane's conversation.
  const childSessions = new Set<string>();
  // Sessions that were already running when the root was deleted. They are its
  // descendants, so none of them may inherit the empty root slot -- only a
  // session started afterwards can be what the pane is now about.
  const ineligibleRoots = new Set<string>();

  // Give the pane's session slot to `sessionID`, unless it is a subagent's.
  //
  // `createdTopLevel` marks positive proof from `session.created`: no parent
  // id, so it is a new conversation and may claim the slot even if it was
  // previously written off as a descendant.
  function claimRoot(sessionID: string, createdTopLevel = false): boolean {
    if (childSessions.has(sessionID)) {
      return false;
    }
    if (!createdTopLevel && ineligibleRoots.has(sessionID)) {
      return false;
    }

    ineligibleRoots.delete(sessionID);
    if (rootSession === sessionID) {
      return false;
    }

    rootSession = sessionID;
    return true;
  }

  async function reportAggregateStatus() {
    const statuses = [...statusBySession.values()];
    let status = 'done';

    if (statuses.includes('waiting')) {
      status = 'waiting';
    } else if (statuses.includes('working')) {
      status = 'working';
    }

    if (reportedStatus === status && reportedSession === rootSession) {
      return;
    }

    reportedStatus = status;
    reportedSession = rootSession;
    if (rootSession) {
      await $`workmux set-window-status ${status} --session-id ${rootSession}`.quiet();
    } else {
      await $`workmux set-window-status ${status}`.quiet();
    }
  }

  function recordStatus(sessionID: string, status: string): boolean {
    const previous = statusBySession.get(sessionID);
    // Ignore the final stale `busy` OpenCode sometimes emits after a session is
    // already done. The next user message re-arms `working` for the new turn.
    if (status === 'working' && acceptBusyBySession.get(sessionID) === false) {
      return false;
    }
    if (previous === status) {
      return false;
    }

    statusBySession.set(sessionID, status);
    if (status === 'done') {
      acceptBusyBySession.set(sessionID, false);
    } else {
      acceptBusyBySession.set(sessionID, true);
    }

    return true;
  }

  async function setStatus(
    sessionID: string | undefined,
    status: string,
  ) {
    if (!sessionID || deletedSessions.has(sessionID)) {
      return;
    }

    // A session seen while no root is known was created before this plugin
    // loaded, i.e. the conversation the client resumed. Sessions created
    // afterwards claim the slot through `session.created` or a user message,
    // never through activity alone: a finished session still emits a trailing
    // `idle` and must not take the pane back from its successor.
    const rootChanged = !rootSession && claimRoot(sessionID);

    // A `done` for a session never seen active says nothing about the pane --
    // OpenCode emits these for conversations that predate this plugin. The
    // aggregate would be empty and report a false completion, so stop here;
    // the root claim above still stands.
    if (status === 'done' && !statusBySession.has(sessionID)) {
      return;
    }

    const statusChanged = recordStatus(sessionID, status);

    if (statusChanged || rootChanged) {
      await reportAggregateStatus();
    }
  }

  return {
    event: async ({ event }) => {
      if (event.type === 'message.updated' && event.properties.info.role === 'user') {
        const sessionID = event.properties.sessionID;
        acceptBusyBySession.set(sessionID, true);
        // The user is talking to this session, so the pane is about it. Covers
        // switching back to an earlier conversation, which creates nothing.
        if (claimRoot(sessionID)) {
          await reportAggregateStatus();
        }
      }

      switch (event.type) {
        case 'session.created':
          // A session with a parent is a subagent's and never owns the pane.
          // One without is a new conversation in this client -- `/new` leaves
          // the previous session alive, so waiting for a deletion would pin
          // the pane to the first conversation for the whole process.
          if (event.properties.info.parentID) {
            childSessions.add(event.properties.info.id);
            break;
          }
          if (claimRoot(event.properties.info.id, true)) {
            await reportAggregateStatus();
          }
          break;
        case 'session.status':
          if (event.properties.status.type === 'busy') {
            await setStatus(event.properties.sessionID, 'working');
          }
          if (event.properties.status.type === 'idle') {
            await setStatus(event.properties.sessionID, 'done');
          }
          break;
        case 'permission.asked':
        case 'question.asked':
          await setStatus(event.properties.sessionID, 'waiting');
          break;
        case 'permission.replied':
        case 'question.replied':
          await setStatus(event.properties.sessionID, 'working');
          break;
        case 'session.idle':
          await setStatus(event.properties.sessionID, 'done');
          break;
        case 'session.deleted': {
          const sessionID = event.properties.info.id;
          deletedSessions.add(sessionID);
          acceptBusyBySession.delete(sessionID);
          childSessions.delete(sessionID);
          const tracked = statusBySession.delete(sessionID);
          // Losing the root session frees the slot for a session started
          // later -- but not for one of its own children, which are still
          // being tracked and would otherwise claim it on their next event.
          if (rootSession === sessionID) {
            rootSession = undefined;
            for (const id of statusBySession.keys()) {
              ineligibleRoots.add(id);
            }
          }
          if (tracked) {
            await reportAggregateStatus();
          }
          break;
        }
      }
    },
  };
};
