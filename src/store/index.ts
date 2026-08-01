import { create } from 'zustand';

import { createProjectSlice, type ProjectSlice } from './projectSlice';
import { createSessionSlice, type SessionSlice } from './sessionSlice';

export type AppStore = ProjectSlice & SessionSlice;

export const useAppStore = create<AppStore>()((...a) => ({
  ...createProjectSlice(...a),
  ...createSessionSlice(...a),
}));
