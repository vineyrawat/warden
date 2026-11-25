import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

// Pill-shaped status badge; tones match the mockup's .p-red/.p-orange/... classes.
const badgeVariants = cva(
  "inline-flex items-center gap-1 whitespace-nowrap rounded-full px-2 py-0.5 text-2xs font-semibold leading-[1.45]",
  {
    variants: {
      tone: {
        red: "bg-red-soft text-red",
        orange: "bg-orange-soft text-orange",
        yellow: "bg-yellow-soft text-yellow",
        green: "bg-green-soft text-green",
        blue: "bg-brand-soft text-brand",
        grey: "bg-surface-2 text-muted-foreground",
        outline: "border bg-card text-foreground",
      },
    },
    defaultVariants: { tone: "grey" },
  },
);

export type BadgeTone = NonNullable<VariantProps<typeof badgeVariants>["tone"]>;

export interface BadgeProps extends React.HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badgeVariants> {}

function Badge({ className, tone, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />;
}

export { Badge, badgeVariants };
