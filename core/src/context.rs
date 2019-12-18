//! Contexts and helper types passed between functions.
use crate::avm1;

use crate::avm1::listeners::SystemListener;
use crate::avm1::Value;
use crate::backend::{audio::AudioBackend, navigator::NavigatorBackend, render::RenderBackend};
use crate::library::Library;
use crate::prelude::*;
use crate::tag_utils::SwfSlice;
use crate::transform::TransformStack;
use core::fmt;
use gc_arena::{Collect, MutationContext};
use rand::rngs::SmallRng;
use std::sync::Arc;

/// `UpdateContext` holds shared data that is used by the various subsystems of Ruffle.
/// `Player` crates this when it begins a tick and passes it through the call stack to
/// children and the VM.
pub struct UpdateContext<'a, 'gc, 'gc_context> {
    /// The queue of actions that will be run after the display list updates.
    /// Display objects and actions can push actions onto the queue.
    pub action_queue: &'a mut ActionQueue<'gc>,

    /// The background color of the Stage. Changed by the `SetBackgroundColor` SWF tag.
    /// TODO: Move this into a `Stage` display object.
    pub background_color: &'a mut Color,

    /// The mutation context to allocate and mutate `GcCell` types.
    pub gc_context: MutationContext<'gc, 'gc_context>,

    /// The time elapsed since this SWF started executing.
    /// Used by AVM1 `GetTime` action and `getTimer` function.
    pub global_time: u64,

    /// The library containing character definitions for this SWF.
    /// Used to instantiate a `DisplayObject` of a given ID.
    pub library: &'a mut Library<'gc>,

    /// The version of the Flash Player we are emulating.
    /// TODO: This is a little confusing because this represents the player's max SWF version,
    /// which is an integer (e.g. 13), but "Flash Player version" is a triplet (11.6.0), and these
    /// aren't in sync. It may be better to have separate `player_swf_version` and `player_version`
    /// variables.
    pub player_version: u8,

    /// The version of the SWF file we are running.
    pub swf_version: u8,

    /// The raw data of the SWF file.
    pub swf_data: &'a Arc<Vec<u8>>,

    /// The audio backend, used by display objects and AVM to play audio.
    pub audio: &'a mut dyn AudioBackend,

    /// The navigator backend, used by the AVM to make HTTP requests and visit webpages.
    pub navigator: &'a mut dyn NavigatorBackend,

    /// The renderer, used by the display objects to draw themselves.
    pub renderer: &'a mut dyn RenderBackend,

    /// The RNG, used by the AVM `RandomNumber` opcode,  `Math.random(),` and `random()`.
    pub rng: &'a mut SmallRng,

    /// The root of the current timeline.
    /// This will generally be `_level0`, except for loadMovie/loadMovieNum.
    pub root: DisplayObject<'gc>,

    /// The `DisplayObject` that this code is running in.
    /// Used by all `DisplayObject` methods and AVM1 `GetVariable`/`SetVariable`/`this`.
    /// Can be changed via a `SetTarget` action.
    /// This will be `root` after an invalid tell target.
    pub active_clip: DisplayObject<'gc>,

    /// Target clip used by Flash 4-era actions (`Play` etc.)
    /// Can be changed via a `SetTarget` action.
    /// This is the same as `active_clip` except in the cases of an invalid tell target:
    /// This is `None` after an invalid tell target.
    pub target_clip: Option<DisplayObject<'gc>>,

    /// The initial target clip.
    /// `target_clip` will reset to this value with `SetTarget ""` action.
    pub start_clip: DisplayObject<'gc>,

    /// The current set of system-specified prototypes to use when constructing
    /// new built-in objects.
    pub system_prototypes: avm1::SystemPrototypes<'gc>,

    /// The display object that the mouse is currently hovering over.
    pub mouse_hovered_object: Option<DisplayObject<'gc>>,

    /// The location of the mouse when it was last over the player.
    pub mouse_position: &'a (Twips, Twips),

    /// The dimensions of the stage.
    pub stage_size: (Twips, Twips),
}

/// A queued ActionScript call.
pub struct QueuedActions<'gc> {
    /// The movie clip this ActionScript is running on.
    pub clip: DisplayObject<'gc>,

    /// The type of action this is, along with the corresponding bytecode/method data.
    pub action_type: ActionType<'gc>,

    /// Whether this is an unload action, which can still run if the clip is removed.
    pub is_unload: bool,
}

unsafe impl<'gc> Collect for QueuedActions<'gc> {
    #[inline]
    fn trace(&self, cc: gc_arena::CollectionContext) {
        self.clip.trace(cc);
        self.action_type.trace(cc);
    }
}

/// Action and gotos need to be queued up to execute at the end of the frame.
pub struct ActionQueue<'gc> {
    queue: std::collections::VecDeque<QueuedActions<'gc>>,
}

impl<'gc> ActionQueue<'gc> {
    const DEFAULT_CAPACITY: usize = 32;

    /// Crates a new `ActionQueue` with an empty queue.
    pub fn new() -> Self {
        Self {
            queue: std::collections::VecDeque::with_capacity(Self::DEFAULT_CAPACITY),
        }
    }

    /// Queues ActionScript to run for the given movie clip.
    /// `actions` is the slice of ActionScript bytecode to run.
    /// The actions will be skipped if the clip is removed before the actions run.
    pub fn queue_actions(
        &mut self,
        clip: DisplayObject<'gc>,
        action_type: ActionType<'gc>,
        is_unload: bool,
    ) {
        self.queue.push_back(QueuedActions {
            clip,
            action_type,
            is_unload,
        })
    }

    /// Pops the next actions off of the queue.
    pub fn pop(&mut self) -> Option<QueuedActions<'gc>> {
        self.queue.pop_front()
    }
}

impl<'gc> Default for ActionQueue<'gc> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<'gc> Collect for ActionQueue<'gc> {
    #[inline]
    fn trace(&self, cc: gc_arena::CollectionContext) {
        self.queue.iter().for_each(|o| o.trace(cc));
    }
}

/// Shared data used during rendering.
/// `Player` creates this when it renders a frame and passes it down to display objects.
pub struct RenderContext<'a, 'gc> {
    /// The renderer, used by the display objects to draw themselves.
    pub renderer: &'a mut dyn RenderBackend,

    /// The library, which provides access to fonts and other definitions when rendering.
    pub library: &'a Library<'gc>,

    /// The transform stack controls the matrix and color transform as we traverse the display hierarchy.
    pub transform_stack: &'a mut TransformStack,
    /// The bounds of the current viewport in twips. Used for culling.
    pub view_bounds: BoundingBox,

    /// The stack of clip depths, used in masking.
    pub clip_depth_stack: Vec<Depth>,
}

/// The type of action being run.
#[derive(Clone)]
pub enum ActionType<'gc> {
    /// Normal frame or event actions.
    Normal { bytecode: SwfSlice },

    /// A `DoInitAction` action.
    Init { bytecode: SwfSlice },

    /// An event handler method, e.g. `onEnterFrame`.
    Method { name: &'static str },

    /// A system listener method,
    NotifyListeners {
        listener: SystemListener,
        method: &'static str,
        args: Vec<Value<'gc>>,
    },
}

impl fmt::Debug for ActionType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ActionType::Normal { bytecode } => f
                .debug_struct("ActionType::Normal")
                .field("bytecode", bytecode)
                .finish(),
            ActionType::Init { bytecode } => f
                .debug_struct("ActionType::Init")
                .field("bytecode", bytecode)
                .finish(),
            ActionType::Method { name } => f
                .debug_struct("ActionType::Method")
                .field("name", name)
                .finish(),
            ActionType::NotifyListeners {
                listener,
                method,
                args,
            } => f
                .debug_struct("ActionType::NotifyListeners")
                .field("listener", listener)
                .field("method", method)
                .field("args", args)
                .finish(),
        }
    }
}

unsafe impl<'gc> Collect for ActionType<'gc> {
    #[inline]
    fn trace(&self, cc: gc_arena::CollectionContext) {
        if let ActionType::NotifyListeners { args, .. } = self {
            args.trace(cc);
        }
    }
}
