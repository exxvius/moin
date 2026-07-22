import { DownloadItem } from "../components/DownloadItem";
import { useStore } from "../lib/store";

export function DownloadsView() {
  const store = useStore();

  return (
    <div className="view">
      <div className="view-head">
        <h2>Downloads</h2>
        <p>Everything currently in flight.</p>
      </div>

      {store.active.length === 0 ? (
        <div className="card">
          <div className="card-title">Nothing downloading</div>
          <p className="dim">Add a link and it'll show up here.</p>
        </div>
      ) : (
        <div className="dl-list">
          {store.active.map((task) => (
            <DownloadItem
              key={task.id}
              task={task}
              speed={store.speeds[task.id]}
              onPause={store.pause}
              onResume={store.resume}
              onCancel={store.cancel}
              onRemove={store.remove}
            />
          ))}
        </div>
      )}
    </div>
  );
}
