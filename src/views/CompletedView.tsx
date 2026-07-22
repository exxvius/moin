import { DownloadItem } from "../components/DownloadItem";
import { useStore } from "../lib/store";

export function CompletedView() {
  const store = useStore();

  return (
    <div className="view">
      <div className="view-head">
        <h2>Completed</h2>
        <p>Finished, failed, and canceled downloads.</p>
      </div>

      {store.finished.length === 0 ? (
        <div className="card">
          <div className="card-title">No history yet</div>
          <p className="dim">Finished downloads land here.</p>
        </div>
      ) : (
        <div className="dl-list">
          {store.finished.map((task) => (
            <DownloadItem
              key={task.id}
              task={task}
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
