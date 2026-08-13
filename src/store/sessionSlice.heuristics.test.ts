// M3-3 が流し込む「推定由来」の StateReason（silence_timeout / bel_detected /
// output_activity）を runtimeReasons が規約どおり運ぶことを固定するテスト。
//
// 既存の src/store/sessionSlice.test.ts:343-426 とテストの形は重なるが、そちらが
// 使う理由は spawned / hook_notification / hook_stop の 3 つ（すべて権威由来）だけで、
// M3-3 がこのフェーズで送信元を作った推定由来の理由を 1 度も通していない。
// 本ファイルはその経路を固定する。
import { beforeEach, describe, expect, it } from 'vitest';
import { useAppStore } from './index';
import type { SessionStatePayload, StateReason } from '../types/model';

const payload = (
  session_id: string,
  runtime_state: SessionStatePayload['runtime_state'],
  reason: SessionStatePayload['reason'],
): SessionStatePayload => ({ session_id, runtime_state, reason });

describe('runtimeReasons', () => {
  beforeEach(() => {
    useAppStore.setState({ runtimeStates: {}, runtimeReasons: {}, runtimeErrors: {} });
  });

  it('starts empty', () => {
    expect(useAppStore.getState().runtimeReasons).toEqual({});
  });

  it('records the reason alongside the state', () => {
    useAppStore.getState().applyStateEvent(payload('s1', 'idle', 'silence_timeout'));
    expect(useAppStore.getState().runtimeStates.s1).toBe('idle');
    expect(useAppStore.getState().runtimeReasons.s1).toBe('silence_timeout');
  });

  it('overwrites a heuristic reason when an authoritative one arrives', () => {
    const store = useAppStore.getState();
    store.applyStateEvent(payload('s1', 'idle', 'silence_timeout'));
    store.applyStateEvent(payload('s1', 'idle', 'hook_stop'));
    expect(useAppStore.getState().runtimeReasons.s1).toBe('hook_stop');
  });

  it('keeps reasons per session independent', () => {
    const store = useAppStore.getState();
    store.applyStateEvent(payload('s1', 'waiting_input', 'bel_detected'));
    store.applyStateEvent(payload('s2', 'waiting_input', 'hook_notification'));
    expect(useAppStore.getState().runtimeReasons).toEqual({
      s1: 'bel_detected',
      s2: 'hook_notification',
    });
  });

  it('accepts every reason defined by the contract', () => {
    // 1 値でも欠けると tsc が落ちる。配列リテラルでは新しい StateReason が
    // 増えても更新が強制されないため（網羅を名乗るテストが実際には掃かない形になる）。
    const ALL_REASONS: Record<StateReason, true> = {
      spawned: true,
      hook_notification: true,
      hook_stop: true,
      pty_exited: true,
      startup_normalize: true,
      bel_detected: true,
      silence_timeout: true,
      user_stopped: true,
      output_activity: true,
      user_input: true,
      hook_permission: true,
      resume_failed: true,
      spawn_failed: true,
    };
    const reasons = Object.keys(ALL_REASONS) as StateReason[];
    for (const reason of reasons) {
      useAppStore.getState().applyStateEvent(payload('s1', 'running', reason));
      expect(useAppStore.getState().runtimeReasons.s1).toBe(reason);
    }
  });
});
