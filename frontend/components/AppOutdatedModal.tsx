import { openExternal } from "../openExternal";

interface AppOutdatedModalProps {
  currentVersion: string;
  requiredVersion: string;
  downloaderAppUrl: string;
}

function AppOutdatedModal({ currentVersion, requiredVersion, downloaderAppUrl }: AppOutdatedModalProps) {
  const handleOpenDownloadPage = () => {
    void openExternal(downloaderAppUrl);
  };

  return (
    <div className="fixed inset-0 bg-black/80 flex items-center justify-center z-50">
      <div className="bg-zinc-800 border border-red-600 rounded-lg p-6 max-w-md mx-4 shadow-xl">
        <div className="flex items-start gap-3 mb-4">
          <svg
            className="h-6 w-6 text-red-400 flex-shrink-0 mt-0.5"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth={2}
              d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"
            />
          </svg>
          <h2 className="text-lg font-semibold text-red-400">Update Required</h2>
        </div>

        <div className="space-y-3 text-sm text-zinc-300 mb-6">
          <p>
            Your version of this app is outdated and no longer compatible with the texture repository.
          </p>
          <div className="bg-zinc-900/50 rounded-lg p-3 space-y-1">
            <p className="flex justify-between">
              <span className="text-zinc-400">Your version:</span>
              <span className="font-mono text-red-400">{currentVersion}</span>
            </p>
            <p className="flex justify-between">
              <span className="text-zinc-400">Required version:</span>
              <span className="font-mono text-green-400">{requiredVersion}+</span>
            </p>
          </div>
          <p>
            Please download the latest version to continue.
          </p>
        </div>

        <button
          onClick={handleOpenDownloadPage}
          className="w-full py-2.5 bg-blue-600 hover:bg-blue-500 text-white font-medium rounded-lg transition-colors"
        >
          Download Latest Version
        </button>
      </div>
    </div>
  );
}

export default AppOutdatedModal;
