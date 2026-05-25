// TODO(ai-review): review for style and correctness
/// Identity of a file inside a specific manifest. Shared between the
/// preview components so the transformer query keys line up.
export type FileLocator = {
  appid: number;
  depotId: number;
  manifestId: string;
  branch: string;
  path: string;
};
