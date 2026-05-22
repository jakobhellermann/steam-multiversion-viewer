// TODO(ai-review): review for style and correctness
import { Outlet, createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/apps/$appid")({
  component: () => <Outlet />,
});
