use {
    infinite_stream::{
        InfiniteStreamExt as _,
        StreamExt as _,
    },
    mhstatus::SubsystemStatusKind,
    tokio_stream::wrappers::{
        BroadcastStream,
        errors::BroadcastStreamRecvError,
    },
    crate::prelude::*,
};

static IMPORT_TASK_STATUS: LazyLock<ParkingLotMutex<(SubsystemStatusKind, broadcast::Sender<SubsystemStatusKind>)>> = LazyLock::new(|| ParkingLotMutex::new((SubsystemStatusKind::Crashed, broadcast::Sender::new(256))));

pub(crate) fn update_import_task_status(new_kind: SubsystemStatusKind) {
    lock!(@sync status = IMPORT_TASK_STATUS; {
        let (kind, tx) = &mut *status;
        *kind = new_kind;
        tx.send(new_kind).allow_unreceived();
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ImportTaskStatus;

impl ctrlflow::Key for ImportTaskStatus {
    type State = Result<SubsystemStatusKind, BroadcastStreamRecvError>;

    fn maintain(&self) -> ctrlflow::Maintenance<Self> {
        ctrlflow::Maintenance::Stream(Box::new(|_| {
            lock!(@sync status = IMPORT_TASK_STATUS; {
                let (init, tx) = &mut *status;
                stream::iter(iter::once(Ok(*init)))
                .chain_infinite(
                    BroadcastStream::new(tx.subscribe())
                        .expect("import task status sender dropped")
                        .map(|update| { eprintln!("import task status update: {update:?}"); update })
                )
                .boxed()
            })
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Sequence)]
pub(crate) enum Subsystem {
    RaceImports,
}

impl fmt::Display for Subsystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RaceImports => write!(f, "race imports"),
        }
    }
}

impl ctrlflow::Key for Subsystem {
    type State = Result<SubsystemStatusKind, BroadcastStreamRecvError>;

    fn maintain(&self) -> ctrlflow::Maintenance<Self> {
        match self {
            Self::RaceImports => ctrlflow::filter_eq(|deps, _| deps.get_latest(ImportTaskStatus).boxed()),
        }
    }
}
