// @prompting-time/ui: Solid 2.0 components that read Sigil's CSS variables. Import
// "@prompting-time/ui/styles.css" once per page for the fonts, the theme, and the components' styles.
export { Button, IconButton, type ButtonProps, type IconButtonProps } from "./Button";
export { Checkbox, type CheckboxProps } from "./Checkbox";
export { CommandPalette, type CommandPaletteProps, type PaletteGroup, type PaletteItem } from "./CommandPalette";
export { ConfirmDialog, type ConfirmDialogProps } from "./ConfirmDialog";
export { CopyButton, type CopyButtonProps } from "./CopyButton";
export { Diff, type DiffMark, type DiffProps, type DiffSource, type DiffTarget } from "./Diff";
export { excerptFile, type LineRange } from "./excerpt";
export { DIFF_THEME, diffTheme, registerDiffTheme } from "./diffTheme";
export { NumberField, TextField, type FieldProps, type NumberFieldProps, type TextFieldProps } from "./Field";
export * from "./icons";
export { ListRow, type ListRowProps } from "./ListRow";
export { Markdown, type MarkdownProps } from "./Markdown";
export { Menu, type MenuItem, type MenuProps } from "./Menu";
export { Select, type SelectOption, type SelectProps } from "./Select";
export { StatusDot, type Status, type StatusDotProps } from "./StatusDot";
export { changedSettings, fieldKey, formValue, leafFields, rangeError, settingFields, settingValue, valueAt, type FormValue, type LeafField, type SettingField } from "./settingsForm";
export { Tabs, type Tab, type TabsProps } from "./Tabs";
export { Textarea, type TextareaProps } from "./Textarea";
export { tokens } from "./theme/tokens";
export { parsePatchFiles, type DiffLineAnnotation, type FileContents, type FileDiffMetadata } from "@pierre/diffs";
