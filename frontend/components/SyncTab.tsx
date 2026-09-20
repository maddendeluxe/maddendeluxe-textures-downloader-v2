import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openExternal } from "../openExternal";
import SyncProgress from "./SyncProgress";
import SyncWarningDialog from "./SyncWarningDialog";

interface SyncStatusResult {
  latest_commit_sha: string;
  latest_commit_date: string;
  last_sync_commit: string | null;
  has_changes: boolean;
}

interface SyncResult {
  files_downloaded: number;
  files_deleted: number;
  files_renamed: number;
  files_skipped: number;
  new_commit_sha: string;
}

interface SyncProgressPayload {
  stage: string;
  message: string;
  current: number | null;
  total: number | null;
}

interface QuickCheckResult {
  local_count: number;
  remote_count: number;
  counts_match: boolean;
}

interface SyncFile {
  path: string;
  to_disabled: boolean;
}

interface SyncAnalysis {
  files_to_add: SyncFile[];
  files_to_replace: SyncFile[];
  files_to_delete: string[];
  commit_sha: string;
}

type SyncStatus = "idle" | "checking" | "syncing" | "complete" | "error";
type SyncMode = "incremental" | "full";

interface SyncTabProps {
  texturesDir: string;
  lastSyncCommit: string | null;
  lastSyncTimestamp: string | null;
  githubToken: string | null;
  onSyncComplete: (commitSha: string) => void;
  onTokenChange: (token: string) => void;
}

// Format ISO date string to human-readable format
const TOKEN_URL =
  "https://github.com/settings/personal-access-tokens/new?name=Textures+Downloader&description=Token+for+syncing+textures&expires_in=365";

// A plain target="_blank" link does nothing inside the Tauri webview: there is no
// browser to open a tab in, so the click was silently dropped. Hand the URL to the
// system browser, exactly as the header's repository link does.
const openTokenPage = (e: React.MouseEvent) => {
  e.preventDefault();
  void openExternal(TOKEN_URL);
};

function formatDate(isoDate: string | null): string {
  if (!isoDate) return "Unknown";
  try {
    const date = new Date(isoDate);
    return date.toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return "Unknown";
  }
}

function SyncTab({
  texturesDir,
  lastSyncCommit,
  lastSyncTimestamp,
  githubToken,
  onSyncComplete,
  onTokenChange,
}: SyncTabProps) {
  const [syncStatus, setSyncStatus] = useState<SyncStatus>("idle");
  const [statusResult, setStatusResult] = useState<SyncStatusResult | null>(null);
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [progressMessages, setProgressMessages] = useState<SyncProgressPayload[]>([]);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [syncMode, setSyncMode] = useState<SyncMode>("incremental");
  const [tokenInput, setTokenInput] = useState(githubToken || "");
  const [showToken, setShowToken] = useState(false);
  const [showOutput, setShowOutput] = useState(false);
  const [tokenSectionExpanded, setTokenSectionExpanded] = useState(!githubToken);
  const [showTokenRequired, setShowTokenRequired] = useState(false);
  const [quickCheckResult, setQuickCheckResult] = useState<QuickCheckResult | null>(null);
  const [pendingAnalysis, setPendingAnalysis] = useState<SyncAnalysis | null>(null);
  const [showWarningDialog, setShowWarningDialog] = useState(false);

  // Listen for sync progress events
  useEffect(() => {
    const unlisten = listen<SyncProgressPayload>("sync-progress", (event) => {
      setProgressMessages((prev) => [...prev, event.payload]);

      if (event.payload.stage === "complete") {
        setSyncStatus("complete");
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Check status when tab is opened or texturesDir changes
  useEffect(() => {
    if (texturesDir) {
      checkSyncStatus();
    }
  }, [texturesDir, githubToken]);

  // Update token input when prop changes
  useEffect(() => {
    setTokenInput(githubToken || "");
    // Collapse section when token is set
    if (githubToken) {
      setTokenSectionExpanded(false);
    }
  }, [githubToken]);

  const checkSyncStatus = async (overrideCommit?: string) => {
    setSyncStatus("checking");
    setErrorMessage(null);

    try {
      const params = {
        texturesDir: texturesDir || "",
        lastSyncCommit: overrideCommit || lastSyncCommit || null,
        githubToken: githubToken || null,
      };

      const result = await invoke<SyncStatusResult>("check_sync_status", params);
      setStatusResult(result);
    } catch (e) {
      console.error("checkSyncStatus error:", e);
      setErrorMessage(`Failed to check status: ${e}`);
    } finally {
      setSyncStatus("idle");
    }
  };

  const handleRunSync = async () => {
    // Check for GitHub token first
    if (!githubToken) {
      setShowTokenRequired(true);
      setTokenSectionExpanded(true);
      return;
    }
    setShowTokenRequired(false);

    setSyncStatus("syncing");
    setProgressMessages([]);
    setSyncResult(null);
    setQuickCheckResult(null);
    setErrorMessage(null);
    setShowOutput(true);

    try {
      if (syncMode === "full") {
        // For full sync: analyze first, then warn if needed
        const analysis = await invoke<SyncAnalysis>("analyze_full_sync", {
          texturesDir,
          githubToken,
        });

        // Check if there are files that will be replaced or deleted
        if (analysis.files_to_replace.length > 0 || analysis.files_to_delete.length > 0) {
          // Show warning dialog and wait for confirmation
          setPendingAnalysis(analysis);
          setShowWarningDialog(true);
          setSyncStatus("idle"); // Pause until user confirms
          return;
        }

        // No warnings needed, proceed directly
        await executeAnalyzedSync(analysis);
      } else {
        // Incremental sync - run directly
        const result = await invoke<SyncResult>("run_sync", {
          texturesDir,
          lastSyncCommit,
          githubToken,
          fullSync: false,
        });

        await finishSync(result);
      }
    } catch (e) {
      setErrorMessage(`Sync failed: ${e}`);
      setSyncStatus("error");
    }
  };

  const executeAnalyzedSync = async (analysis: SyncAnalysis) => {
    setSyncStatus("syncing");
    setShowOutput(true);

    try {
      const result = await invoke<SyncResult>("execute_analyzed_sync", {
        texturesDir,
        filesToAdd: analysis.files_to_add,
        filesToReplace: analysis.files_to_replace,
        filesToDelete: analysis.files_to_delete,
        commitSha: analysis.commit_sha,
        githubToken,
      });

      await finishSync(result);
    } catch (e) {
      setErrorMessage(`Sync failed: ${e}`);
      setSyncStatus("error");
    }
  };

  const finishSync = async (result: SyncResult) => {
    // Run quick count check (fast, no SHA computation)
    try {
      const quickCheck = await invoke<QuickCheckResult>("run_quick_count_check", {
        texturesDir,
        githubToken,
      });
      setQuickCheckResult(quickCheck);
    } catch (countError) {
      console.error("Quick count check failed:", countError);
    }

    setSyncResult(result);
    onSyncComplete(result.new_commit_sha);
    setSyncStatus("complete");
    await checkSyncStatus(result.new_commit_sha);
  };

  const handleWarningConfirm = async () => {
    setShowWarningDialog(false);
    if (pendingAnalysis) {
      await executeAnalyzedSync(pendingAnalysis);
      setPendingAnalysis(null);
    }
  };

  const handleWarningCancel = () => {
    setShowWarningDialog(false);
    setPendingAnalysis(null);
    setSyncStatus("idle");
    setProgressMessages((prev) => [
      ...prev,
      { stage: "cancelled", message: "Sync cancelled by user.", current: null, total: null },
    ]);
  };

  const handleSaveToken = () => {
    onTokenChange(tokenInput);
    if (tokenInput) {
      setShowTokenRequired(false);
    }
  };

  const isSyncing = syncStatus === "syncing";
  const isChecking = syncStatus === "checking";

  return (
    <div className="space-y-4">
      {/* GitHub API Token */}
      <div className="bg-zinc-900 border border-zinc-700 rounded-lg p-4">
        <button
          type="button"
          onClick={() => setTokenSectionExpanded(!tokenSectionExpanded)}
          className="w-full flex items-center justify-between"
        >
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-medium text-zinc-300">GitHub API Token</h3>
            {githubToken && !tokenSectionExpanded && (
              <span className="text-xs text-green-400">configured</span>
            )}
          </div>
          <svg
            className={`h-4 w-4 text-zinc-400 transition-transform ${tokenSectionExpanded ? "rotate-180" : ""}`}
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
          >
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
          </svg>
        </button>
        {tokenSectionExpanded && (
          <div className="mt-3 space-y-3">
            <p className="text-xs text-zinc-500">
              Required. A free GitHub.com account is needed.
              <a
                href={TOKEN_URL}
                onClick={openTokenPage}
                className="text-blue-400 hover:text-blue-300 ml-1 cursor-pointer"
              >
                Generate fine-grained token
              </a>
            </p>
            <div className="flex gap-2">
              <div className="flex-1 relative">
                <input
                  type={showToken ? "text" : "password"}
                  value={tokenInput}
                  onChange={(e) => setTokenInput(e.target.value)}
                  placeholder="ghp_xxxxxxxxxxxx"
                  className="w-full px-3 py-2 bg-zinc-800 border border-zinc-600 rounded text-sm text-zinc-200 placeholder-zinc-500 focus:outline-none focus:border-blue-500"
                />
                <button
                  type="button"
                  onClick={() => setShowToken(!showToken)}
                  className="absolute right-2 top-1/2 -translate-y-1/2 text-zinc-500 hover:text-zinc-300 text-xs"
                >
                  {showToken ? "Hide" : "Show"}
                </button>
              </div>
              <button
                onClick={handleSaveToken}
                disabled={tokenInput === (githubToken || "")}
                className="px-3 py-2 bg-zinc-700 hover:bg-zinc-600 disabled:bg-zinc-800 disabled:text-zinc-600 text-sm rounded transition-colors"
              >
                Save
              </button>
            </div>
          </div>
        )}
      </div>

      {/* Sync Status */}
      <div className="bg-zinc-900 border border-zinc-700 rounded-lg p-4 space-y-3">
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-medium text-zinc-300">Sync Status</h3>
          <button
            onClick={() => checkSyncStatus()}
            disabled={isChecking || isSyncing}
            className="text-xs text-blue-400 hover:text-blue-300 disabled:opacity-50"
          >
            {isChecking ? "Checking..." : "Refresh"}
          </button>
        </div>

        {statusResult && (
          <div className="space-y-2 text-sm">
            <div className="flex justify-between">
              <span className="text-zinc-400">Latest update:</span>
              <span className="text-zinc-300 text-xs">
                {formatDate(statusResult.latest_commit_date)}
              </span>
            </div>
            {lastSyncTimestamp && (
              <div className="flex justify-between">
                <span className="text-zinc-400">Last synced:</span>
                <span className="text-zinc-300 text-xs">
                  {formatDate(lastSyncTimestamp)}
                </span>
              </div>
            )}
            <div className="border-t border-zinc-700 pt-2 mt-2">
              {!statusResult.has_changes ? (
                <div className="flex items-center gap-2 text-green-400">
                  <svg className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                  </svg>
                  <span>Textures are up to date!</span>
                </div>
              ) : (
                <div className="flex items-center gap-2 text-yellow-400">
                  <svg className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                  </svg>
                  <span>Updates available</span>
                </div>
              )}
            </div>
          </div>
        )}

        {isChecking && !statusResult && (
          <div className="flex items-center gap-2 text-zinc-400">
            <svg className="animate-spin h-4 w-4" fill="none" viewBox="0 0 24 24">
              <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
              <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
            </svg>
            <span className="text-sm">Checking for updates...</span>
          </div>
        )}
      </div>

      {/* Sync Mode */}
      <div className="bg-zinc-900 border border-zinc-700 rounded-lg p-4 space-y-3">
        <h3 className="text-sm font-medium text-zinc-300">Sync Mode</h3>
        <div className="space-y-2">
          <label className="flex items-center gap-3 cursor-pointer">
            <input
              type="radio"
              name="syncMode"
              value="incremental"
              checked={syncMode === "incremental"}
              onChange={() => setSyncMode("incremental")}
              disabled={isSyncing}
              className="w-4 h-4 text-blue-500 bg-zinc-700 border-zinc-600"
            />
            <div>
              <span className="text-sm text-zinc-200">Download New Content</span>
              <span className="text-xs text-zinc-500 ml-2">(recommended)</span>
              <p className="text-xs text-zinc-500">Only download changes since last sync</p>
            </div>
          </label>
          <label className="flex items-center gap-3 cursor-pointer">
            <input
              type="radio"
              name="syncMode"
              value="full"
              checked={syncMode === "full"}
              onChange={() => setSyncMode("full")}
              disabled={isSyncing}
              className="w-4 h-4 text-blue-500 bg-zinc-700 border-zinc-600"
            />
            <div>
              <span className="text-sm text-zinc-200">Full Sync</span>
              <p className="text-xs text-zinc-500">Compare all files against repository (slower)</p>
            </div>
          </label>
        </div>
      </div>

      {/* Sync button */}
      <button
        onClick={handleRunSync}
        disabled={!texturesDir || isSyncing || isChecking}
        className={`
          w-full py-3 rounded-lg font-medium transition-all
          ${isSyncing || isChecking
            ? "bg-zinc-700 text-zinc-400 cursor-wait"
            : "bg-blue-600 hover:bg-blue-500 text-white"
          }
        `}
      >
        {isSyncing ? "Syncing..." : syncMode === "full" ? "Run Full Sync" : "Run Sync"}
      </button>

      {/* Token required warning */}
      {showTokenRequired && (
        <div className="p-3 bg-yellow-900/30 border border-yellow-700 rounded text-yellow-300 text-sm">
          A GitHub API key is required for syncing.{" "}
          <a
            href={TOKEN_URL}
            onClick={openTokenPage}
            className="text-blue-400 hover:text-blue-300 underline cursor-pointer"
          >
            Click here to generate an API token
          </a>
          .
        </div>
      )}

      {/* Error message */}
      {errorMessage && (
        <div className="p-3 bg-red-900/30 border border-red-800 rounded text-red-300 text-sm">
          {errorMessage}
        </div>
      )}

      {/* Progress display - stays visible after sync completes */}
      {showOutput && progressMessages.length > 0 && (
        <SyncProgress
          messages={progressMessages}
          isComplete={!isSyncing && syncResult !== null}
          result={syncResult}
        />
      )}

      {/* Quick count check result - show when we have a result and sync is done (status is complete or idle after sync) */}
      {quickCheckResult && syncResult && (syncStatus === "complete" || syncStatus === "idle") && (
        <div className={`p-3 rounded text-sm ${
          quickCheckResult.counts_match
            ? "bg-green-900/30 border border-green-800 text-green-300"
            : "bg-yellow-900/30 border border-yellow-700 text-yellow-300"
        }`}>
          {quickCheckResult.counts_match ? (
            <div className="flex items-center gap-2">
              <svg className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
              <span>File count verified: {quickCheckResult.local_count} files match repository</span>
            </div>
          ) : (
            <div>
              <div className="flex items-center gap-2 mb-1">
                <svg className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                </svg>
                <span>File count mismatch</span>
              </div>
              <p className="text-xs ml-6">
                Local: {quickCheckResult.local_count} files, Repository: {quickCheckResult.remote_count} files.
                Run "Full Sync" to fix discrepancies.
              </p>
            </div>
          )}
        </div>
      )}

      {/* Info about sync behavior */}
      <div className="bg-zinc-900/50 border border-zinc-700 rounded-lg p-3 text-xs text-zinc-500">
        <p className="font-medium text-zinc-400 mb-1">About Sync</p>
        <ul className="space-y-1 list-disc list-inside">
          <li>Files in <code className="text-zinc-400">user-customs/</code> are never modified</li>
          <li>Disabled textures (dash-prefixed) stay disabled but get updated</li>
          <li>Deleted textures are removed (including disabled versions)</li>
          <li>Renamed textures are moved (preserving disabled state)</li>
        </ul>
      </div>

      {/* Warning dialog for files that will be replaced/deleted */}
      {showWarningDialog && pendingAnalysis && (
        <SyncWarningDialog
          filesToReplace={pendingAnalysis.files_to_replace}
          filesToDelete={pendingAnalysis.files_to_delete}
          onConfirm={handleWarningConfirm}
          onCancel={handleWarningCancel}
        />
      )}
    </div>
  );
}

export default SyncTab;
