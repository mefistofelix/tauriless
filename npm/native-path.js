import path from "node:path";
import { fileURLToPath } from "node:url";

const libraries = {
  "darwin-x64": "libtauriless.dylib",
  "linux-x64": "libtauriless.so",
  "win32-x64": "tauriless.dll",
};

export function getNativeLibraryPath(
  platform = process.platform,
  arch = process.arch,
) {
  if (process.env.TAURILESS_LIBRARY_PATH) {
    return path.resolve(process.env.TAURILESS_LIBRARY_PATH);
  }

  const target = `${platform}-${arch}`;
  const filename = libraries[target];
  if (!filename) {
    throw new Error(
      `Tauriless does not ship a binary for ${target}; supported targets are ${
        Object.keys(libraries).join(", ")
      }`,
    );
  }

  return fileURLToPath(
    new URL(`./native/${target}/${filename}`, import.meta.url),
  );
}

export const nativeLibraryPath = getNativeLibraryPath();
