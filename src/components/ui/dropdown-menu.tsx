import * as Dropdown from "@radix-ui/react-dropdown-menu";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export const DropdownMenu = Dropdown.Root;
export const DropdownMenuTrigger = Dropdown.Trigger;
export const DropdownMenuGroup = Dropdown.Group;

export function DropdownMenuContent({
  className,
  ...props
}: ComponentProps<typeof Dropdown.Content>) {
  return (
    <Dropdown.Portal>
      <Dropdown.Content
        sideOffset={6}
        className={cn(
          "z-50 min-w-44 overflow-hidden rounded-lg bg-surface p-1 text-sm shadow-[var(--shadow-border)]",
          className,
        )}
        {...props}
      />
    </Dropdown.Portal>
  );
}

export function DropdownMenuItem({
  className,
  ...props
}: ComponentProps<typeof Dropdown.Item>) {
  return (
    <Dropdown.Item
      className={cn(
        "flex cursor-pointer items-center gap-2 rounded-md px-2.5 py-2 outline-none data-[highlighted]:bg-surface-2",
        className,
      )}
      {...props}
    />
  );
}

export function DropdownMenuSeparator() {
  return <Dropdown.Separator className="my-1 h-px bg-border" />;
}

export function DropdownMenuLabel({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("px-2.5 py-1.5 text-[11px] font-medium tracking-wide text-faint uppercase", className)} {...props} />;
}
