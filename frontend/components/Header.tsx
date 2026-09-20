import { APP_TITLE, REPO_NAME, REPO_URL, TARGET_FOLDER } from "../config";
import { openExternal } from "../openExternal";

interface HeaderProps {
  version?: string;
}

function Header({ version }: HeaderProps) {
  const handleRepoClick = (e: React.MouseEvent) => {
    e.preventDefault();
    void openExternal(REPO_URL);
  };

  return (
    <header className="text-center">
      <h1 className="text-2xl font-bold text-zinc-100">
        {APP_TITLE}
        {version && (
          <span className="text-sm font-normal text-zinc-500 ml-2">v{version}</span>
        )}
      </h1>
      <p className="text-sm text-zinc-400 mt-1">
        <a
          href={REPO_URL}
          onClick={handleRepoClick}
          className="hover:text-blue-400 hover:underline transition-colors cursor-pointer"
        >
          {REPO_NAME}
        </a>
        <span className="text-zinc-600 mx-2">|</span>
        {TARGET_FOLDER}
      </p>
    </header>
  );
}

export default Header;
