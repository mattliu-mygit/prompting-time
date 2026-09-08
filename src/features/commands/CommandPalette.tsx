import * as Dialog from "@radix-ui/react-dialog";
import { Command } from "cmdk";
import { useRef } from "react";

type PaletteConversation = {
  id: string;
  title: string;
  projectRoot: string | null;
};

type PaletteCommand = {
  id: string;
  label: string;
  disabled?: boolean;
  run(): void;
};

export default function CommandPalette({ open, onOpenChange, conversations, selectedId, onSelectConversation, commands, agents = [], onSelectAgent }: {
  open: boolean;
  onOpenChange(open: boolean): void;
  conversations: readonly PaletteConversation[];
  selectedId: string | null;
  onSelectConversation(id: string): void;
  commands: readonly PaletteCommand[];
  agents?: readonly { conversationId: string; conversationTitle: string; agent: { id: string; label: string } }[];
  onSelectAgent?(conversationId: string, agentId: string): void;
}) {
  const search = useRef<HTMLInputElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const afterClose = useRef<(() => void) | null>(null);

  function choose(action: () => void) {
    afterClose.current = action;
    onOpenChange(false);
  }

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="palette-overlay" />
        <Dialog.Content
          className="command-palette"
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
            search.current?.focus();
          }}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            // Run commands after the palette releases its focus scope.
            const action = afterClose.current;
            afterClose.current = null;
            returnFocus.current?.focus();
            action?.();
          }}
        >
          <Dialog.Title className="sr-only">Search conversations and commands</Dialog.Title>
          <Dialog.Description className="sr-only">Find active conversations by title or project path, inspect known current-run agents, or choose a command.</Dialog.Description>
          <Command label="Search conversations and commands" vimBindings={false}>
            <Command.Input ref={search} placeholder="Search conversations and commands…" />
            <Command.List label="Conversations and commands">
              <Command.Empty>No conversations or commands found.</Command.Empty>
              <Command.Group heading="Conversations">
                {conversations.map((conversation) => (
                  <Command.Item
                    key={conversation.id}
                    value={`conversation:${conversation.id}`}
                    keywords={[conversation.title, conversation.projectRoot ?? ""]}
                    onSelect={() => choose(() => onSelectConversation(conversation.id))}
                  >
                    <span className="palette-conversation">
                      <span>{conversation.title}</span>
                      {conversation.projectRoot ? <small>{conversation.projectRoot}</small> : null}
                    </span>
                    {selectedId === conversation.id ? <span className="palette-current">Current</span> : null}
                  </Command.Item>
                ))}
              </Command.Group>
              {onSelectAgent ? <Command.Group heading="Agents">
                {agents.map(({ conversationId, conversationTitle, agent }) => <Command.Item
                  key={`${conversationId}:${agent.id}`}
                  value={`agent:${conversationId}:${agent.id}`}
                  aria-label={`${agent.label} ${conversationTitle}`}
                  keywords={[`${agent.label} ${conversationTitle}`]}
                  onSelect={() => choose(() => onSelectAgent(conversationId, agent.id))}
                >
                  <span className="palette-conversation"><span>{agent.label}</span><small>{conversationTitle}</small></span>
                </Command.Item>)}
              </Command.Group> : null}
              <Command.Group heading="Commands">
                {commands.map((command) => (
                  <Command.Item
                    key={command.id}
                    value={`command:${command.id}`}
                    keywords={[command.label]}
                    disabled={command.disabled}
                    onSelect={() => { if (!command.disabled) choose(command.run); }}
                  >{command.label}</Command.Item>
                ))}
              </Command.Group>
            </Command.List>
          </Command>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
