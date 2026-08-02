import { beforeEach, describe, expect, it, vi } from 'vitest';

// vi.mock はファイル先頭に巻き上げられるので、モック関数は vi.hoisted で先に作る
const mocks = vi.hoisted(() => ({
  unlistenData: vi.fn(),
  unlistenExit: vi.fn(),
  onPtyData: vi.fn(),
  onPtyExit: vi.fn(),
  ensureTerminal: vi.fn(),
  writeToTerminal: vi.fn(),
  writeNotice: vi.fn(),
}));

vi.mock('../ipc/events', () => ({
  onPtyData: mocks.onPtyData,
  onPtyExit: mocks.onPtyExit,
}));
vi.mock('./registry', () => ({
  ensureTerminal: mocks.ensureTerminal,
  writeToTerminal: mocks.writeToTerminal,
  writeNotice: mocks.writeNotice,
}));

import {
  disposePtySubscription,
  ensurePtySubscription,
  isStarted,
  markStarted,
  resetPtyBridgeForTest,
  unmarkStarted,
} from './ptyBridge';

describe('ptyBridge', () => {
  beforeEach(() => {
    resetPtyBridgeForTest();
    vi.clearAllMocks();
    mocks.onPtyData.mockResolvedValue(mocks.unlistenData);
    mocks.onPtyExit.mockResolvedValue(mocks.unlistenExit);
  });

  it('購読前にターミナルを生成し、data と exit の両方を購読する', async () => {
    await ensurePtySubscription('s1:agent');
    expect(mocks.ensureTerminal).toHaveBeenCalledWith('s1:agent');
    expect(mocks.onPtyData).toHaveBeenCalledTimes(1);
    expect(mocks.onPtyExit).toHaveBeenCalledTimes(1);
  });

  it('同じ surface_id を二度呼んでも購読は 1 組だけ（二重表示を防ぐ）', async () => {
    await ensurePtySubscription('s1:agent');
    await ensurePtySubscription('s1:agent');
    expect(mocks.onPtyData).toHaveBeenCalledTimes(1);
    expect(mocks.onPtyExit).toHaveBeenCalledTimes(1);
  });

  it('同じ surface_id を並行して呼んでも購読は 1 組だけ（await 前の二重呼び出し）', async () => {
    // 呼び出し側が await せずに 2 回呼ぶケース（例外的経路だが Promise キャッシュで守る）
    const p1 = ensurePtySubscription('s1:agent');
    const p2 = ensurePtySubscription('s1:agent');
    await Promise.all([p1, p2]);
    expect(mocks.onPtyData).toHaveBeenCalledTimes(1);
    expect(mocks.onPtyExit).toHaveBeenCalledTimes(1);
  });

  it('dispose で両方の listener を解除し、再購読できる', async () => {
    await ensurePtySubscription('s1:agent');
    disposePtySubscription('s1:agent');
    expect(mocks.unlistenData).toHaveBeenCalledTimes(1);
    expect(mocks.unlistenExit).toHaveBeenCalledTimes(1);

    await ensurePtySubscription('s1:agent');
    expect(mocks.onPtyData).toHaveBeenCalledTimes(2);
  });

  it('listen 登録の解決を待たずに dispose しても、後から解決した listener を取りこぼさず解除する', async () => {
    // 不変条件 C の弱点: ready が pending のうちに dispose すると
    // その時点の unlisten 配列は空。あとから解決したハンドルが孤児にならないことを保証する。
    let resolveData: (fn: () => void) => void = () => {};
    mocks.onPtyData.mockReturnValueOnce(
      new Promise<() => void>((resolve) => {
        resolveData = resolve;
      }),
    );

    const ready = ensurePtySubscription('s1:agent');
    disposePtySubscription('s1:agent');
    resolveData(mocks.unlistenData);
    await ready.catch(() => undefined);
    // マイクロタスクを1周させて then チェーンを進める
    await Promise.resolve();
    await Promise.resolve();

    expect(mocks.unlistenData).toHaveBeenCalledTimes(1);
    expect(mocks.unlistenExit).toHaveBeenCalledTimes(1);
  });

  it('onPtyExit の登録だけ失敗しても、onPtyData 側で登録済みの listener を解除する', async () => {
    // Promise.all の部分失敗: data は成功、exit は失敗するケース。
    // 成功済みハンドルを取りこぼすと、再試行後に dispose しても解除されず listener が残る
    // （1 チャンクが N 回書き込まれて「文字が二重に出る」症状になる）。
    const staleDataUnlisten = vi.fn();
    mocks.onPtyData.mockResolvedValueOnce(staleDataUnlisten);
    mocks.onPtyExit.mockRejectedValueOnce(new Error('listen failed'));

    await expect(ensurePtySubscription('s1:agent')).rejects.toThrow('listen failed');
    // 失敗直後、data 側は登録に成功していたのでその場で解除されていること
    expect(staleDataUnlisten).toHaveBeenCalledTimes(1);

    // 失敗後の再試行で data リスナが積み上がらないこと
    mocks.onPtyExit.mockResolvedValueOnce(mocks.unlistenExit);
    await ensurePtySubscription('s1:agent');
    expect(mocks.onPtyData).toHaveBeenCalledTimes(2);

    disposePtySubscription('s1:agent');
    // 最終的に有効なのは 2 回目の登録だけなので、dispose で解除されるのは 1 回きり
    expect(mocks.unlistenData).toHaveBeenCalledTimes(1);
  });

  it('markStarted / isStarted で起動済みを追跡する', () => {
    expect(isStarted('s1:agent')).toBe(false);
    markStarted('s1:agent');
    expect(isStarted('s1:agent')).toBe(true);
  });

  it('unmarkStarted で起動失敗したサーフェスを再試行可能に戻す', () => {
    markStarted('s1:agent');
    unmarkStarted('s1:agent');
    // spawn 失敗時は pty://exit が来ないので、この経路が唯一の回復手段
    expect(isStarted('s1:agent')).toBe(false);
  });

  it('pty://exit を受けると起動済みフラグが落ちて再起動できる', async () => {
    await ensurePtySubscription('s1:agent');
    markStarted('s1:agent');
    // onPtyExit に渡されたハンドラを直接呼ぶ
    const exitHandler = mocks.onPtyExit.mock.calls[0][1] as (payload: {
      surface_id: string;
      exit_code: number | null;
    }) => void;
    exitHandler({ surface_id: 's1:agent', exit_code: 0 });
    expect(isStarted('s1:agent')).toBe(false);
    expect(mocks.writeNotice).toHaveBeenCalledWith('s1:agent', '[process exited: 0]');
  });

  it('pty://data のハンドラは base64 を復号して writeToTerminal に渡す', async () => {
    await ensurePtySubscription('s1:agent');
    const dataHandler = mocks.onPtyData.mock.calls[0][1] as (payload: {
      base64: string;
      seq: number;
    }) => void;
    dataHandler({ base64: 'aGk=', seq: 7 });
    expect(mocks.writeToTerminal).toHaveBeenCalledWith('s1:agent', new Uint8Array([104, 105]), 7);
  });

  it('exit を受けた後に届いた data もそのまま writeToTerminal に渡す（exit をストリーム終端と仮定しない）', async () => {
    await ensurePtySubscription('s1:agent');
    const exitHandler = mocks.onPtyExit.mock.calls[0][1] as (payload: {
      surface_id: string;
      exit_code: number | null;
    }) => void;
    const dataHandler = mocks.onPtyData.mock.calls[0][1] as (payload: {
      base64: string;
      seq: number;
    }) => void;

    exitHandler({ surface_id: 's1:agent', exit_code: 0 });
    dataHandler({ base64: 'aGk=', seq: 99 });

    expect(mocks.writeToTerminal).toHaveBeenCalledWith('s1:agent', new Uint8Array([104, 105]), 99);
  });
});
