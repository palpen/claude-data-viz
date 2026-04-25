declare module "tinykeys" {
  export type KeyBindingHandler = (event: KeyboardEvent) => void;
  export type KeyBindingMap = Record<string, KeyBindingHandler>;
  export interface KeyBindingOptions {
    event?: "keydown" | "keyup";
    capture?: boolean;
    timeout?: number;
  }
  export function tinykeys(
    target: Window | HTMLElement,
    keyBindingMap: KeyBindingMap,
    options?: KeyBindingOptions
  ): () => void;
}
