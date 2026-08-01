import { create } from 'zustand';

import { createProjectSlice, type ProjectSlice } from './projectSlice';
import { createSessionSlice, type SessionSlice } from './sessionSlice';
import { createUiSlice, type UiSlice } from './uiSlice';

export type AppStore = ProjectSlice & SessionSlice & UiSlice;

export const useAppStore = create<AppStore>()((...a) => ({
  ...createProjectSlice(...a),
  ...createSessionSlice(...a),
  ...createUiSlice(...a),
}));
