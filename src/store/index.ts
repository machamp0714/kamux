import { create } from 'zustand';

import { createSessionSlice, type SessionSlice } from './sessionSlice';

export type AppStore = SessionSlice;

export const useAppStore = create<AppStore>()((...a) => ({
  ...createSessionSlice(...a),
}));
