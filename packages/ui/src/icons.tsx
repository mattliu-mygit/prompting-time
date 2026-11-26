// Icons for Prompting Time and plugin pages, drawn in the text color. Paths from Lucide
// (lucide.dev), whose license is in icons.LICENSE.
import type { JSX } from "@solidjs/web";

function Icon(props: { children: JSX.Element }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      {props.children}
    </svg>
  );
}

export const SearchIcon = () => (
  <Icon>
    <path d="m21 21-4.34-4.34" />
    <circle cx="11" cy="11" r="8" />
  </Icon>
);
export const FilterIcon = () => (
  <Icon>
    <path d="M3 6h18" />
    <path d="M7 12h10" />
    <path d="M10 18h4" />
  </Icon>
);
export const EllipsisIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="1" />
    <circle cx="19" cy="12" r="1" />
    <circle cx="5" cy="12" r="1" />
  </Icon>
);
export const ExternalIcon = () => (
  <Icon>
    <path d="M15 3h6v6" />
    <path d="M10 14 21 3" />
    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
  </Icon>
);
export const PanelLeftIcon = () => (
  <Icon>
    <rect width="18" height="18" x="3" y="3" rx="2" />
    <path d="M9 3v18" />
  </Icon>
);
export const PanelRightIcon = () => (
  <Icon>
    <rect width="18" height="18" x="3" y="3" rx="2" />
    <path d="M15 3v18" />
  </Icon>
);
export const PullRequestIcon = () => (
  <Icon>
    <circle cx="18" cy="18" r="3" />
    <circle cx="6" cy="6" r="3" />
    <path d="M13 6h3a2 2 0 0 1 2 2v7" />
    <path d="M6 9v12" />
  </Icon>
);
export const ChatIcon = () => (
  <Icon>
    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
  </Icon>
);
export const BotIcon = () => (
  <Icon>
    <rect width="16" height="12" x="4" y="8" rx="2" />
    <path d="M12 8V4H8" />
    <path d="M9 13v2" />
    <path d="M15 13v2" />
  </Icon>
);
export const CopyIcon = () => (
  <Icon>
    <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
    <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
  </Icon>
);
export const CloseIcon = () => (
  <Icon>
    <path d="M18 6 6 18" />
    <path d="m6 6 12 12" />
  </Icon>
);
export const ChevronRightIcon = () => (
  <Icon>
    <path d="m9 18 6-6-6-6" />
  </Icon>
);
export const ChevronDownIcon = () => (
  <Icon>
    <path d="m6 9 6 6 6-6" />
  </Icon>
);
