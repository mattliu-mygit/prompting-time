import type { Machine, MachineSchema, Service } from "@zag-js/core";
import { normalizeProps, spreadProps, VanillaMachine, type Attrs } from "@zag-js/vanilla";
import { createEffect, createMemo, createSignal, onSettled, untrack } from "solid-js";

/**
 * Runs a Zag.js state machine for the life of the calling component and returns its connected
 * API, which re-reads whenever the machine changes. Zag's framework-free VanillaMachine runs the
 * machine: Zag's own Solid adapter targets Solid 1. When a Solid value read by `props` changes,
 * the machine gets the new props through updateProps.
 */
export function useMachine<T extends MachineSchema, Api>(
  machine: Machine<T>,
  props: () => Partial<T["props"]>,
  connect: (service: Service<T>, normalize: typeof normalizeProps) => Api,
): () => Api {
  const running = new VanillaMachine(machine, untrack(props));
  const [version, setVersion] = createSignal(0);
  createEffect(
    props,
    (next) => {
      running.updateProps(next);
    },
    { defer: true },
  );
  onSettled(() => {
    const unsubscribe = running.subscribe(() => setVersion((count) => count + 1));
    running.start();
    return () => {
      unsubscribe();
      running.stop();
    };
  });
  return createMemo(() => {
    version();
    return connect(running.service, normalizeProps);
  });
}

/**
 * A ref that keeps an element's attributes and event listeners equal to a Zag part's props, such
 * as `api().getTriggerProps()`. spreadProps compares with the props it applied last, so a prop
 * that disappears is removed. The listeners go away with the element when the component unmounts.
 */
export function spread(props: () => Attrs): (element: HTMLElement) => void {
  let target: HTMLElement | undefined;
  createEffect(props, (attrs) => {
    if (target) spreadProps(target, attrs);
  });
  return (element) => {
    target = element;
  };
}
