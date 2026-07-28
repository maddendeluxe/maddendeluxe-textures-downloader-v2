import { open } from "@tauri-apps/plugin-dialog";

interface DirectoryPickerProps {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}

function DirectoryPicker({ value, onChange, disabled }: DirectoryPickerProps) {
  const handleBrowse = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select PCSX2 Textures Directory",
      });

      if (selected && typeof selected === "string") {
        onChange(selected);
      }
    } catch (e) {
      console.error("Failed to open directory picker:", e);
    }
  };

  return (
    <div>
      <label className="block text-sm font-medium text-zinc-300 mb-2">
        PCSX2 Textures Directory
      </label>
      <div className="flex gap-2">
        <input
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="Select your textures directory..."
          disabled={disabled}
          className="flex-1 px-3 py-2 bg-zinc-900 border border-zinc-600 rounded-lg
                     text-zinc-100 placeholder-zinc-500 text-sm
                     focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent
                     disabled:opacity-50 disabled:cursor-not-allowed"
        />
        <button
          onClick={handleBrowse}
          disabled={disabled}
          className="px-4 py-2 bg-zinc-700 hover:bg-zinc-600 text-zinc-100 rounded-lg
                     transition-colors text-sm font-medium
                     disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-zinc-700"
        >
          Browse
        </button>
      </div>
      <p className="mt-1 text-xs text-zinc-500">
        Example: C:\Games\Madden 09 Deluxe\Textures or ~/Library/Application Support/PCSX2/textures
      </p>
    </div>
  );
}

export default DirectoryPicker;
