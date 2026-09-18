/// <reference types="vite/client" />

/// The contents of games/<GAME>/game.json, injected by vite.config.ts at build time.
/// See frontend/config.ts, which is the only place that should read it.
declare const __GAME_CONFIG__: {
  id: string;
  title: string;
  shortName: string;
  description: string;
  identifier: string;
  repoOwner: string;
  repoName: string;
  repoUrl: string;
  slusFolder: string;
  sparsePath: string;
  tempDirName: string;
  examplePath: string;
};
