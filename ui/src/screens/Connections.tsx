// Legacy screens, kept as thin wrappers so old imports keep working. Their cards now live in the
// Settings panes (screens/settings/*): engine → Claude, forges → Accounts, tools → Tools,
// watched projects → Projects.
import { Settings } from "@/screens/Settings";
import { ProjectsPane } from "@/screens/settings/Projects";

export function ConnectionsScreen() {
  return <Settings route={{ screen: "settings", pane: "general" }} />;
}

export function ProjectsScreen() {
  return <Settings route={{ screen: "settings", pane: "projects" }} />;
}

/** The project list + project page from the Projects pane. `className` sizes the wrapper. */
export function WatchedProjects({ className }: { className?: string }) {
  return (
    <div className={className ?? "h-full min-h-0"}>
      <ProjectsPane />
    </div>
  );
}
