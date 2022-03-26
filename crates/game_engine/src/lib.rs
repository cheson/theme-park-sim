use std::time::Instant;

use event::EventManager;
use frame_buffer::FrameBufferManager;
use futures::pin_mut;
use game_controller::GameController;
use game_input::{GameInput, GameInputInterface};
use game_system::FIXED_TIMESTEP;
use task_executor::{parallel, TaskExecutor};
use winit::{event::WindowEvent, window::Window};

use crate::{fixed_update::FixedUpdate, frame_update::FrameUpdateSystems};

mod fixed_update;
mod frame_update;

#[cfg(target_vendor = "apple")]
use metal::Metal;

#[cfg(not(target_vendor = "apple"))]
use vulkan::Vulkan;

pub struct GameEngine {
    event_manager: EventManager,
    fixed_update: FixedUpdate,
    frame_buffer_manager: FrameBufferManager,
    frame_update_systems: FrameUpdateSystems,
    game_controller: GameController,
    input: GameInput,
    last_fixed_update_instant: Instant,
    task_executor: TaskExecutor,

    #[cfg(target_vendor = "apple")]
    graphics: Metal,

    #[cfg(not(target_vendor = "apple"))]
    graphics: Vulkan,
}

impl GameEngine {
    pub fn new(window: &Window) -> Self {
        let thread_count = TaskExecutor::available_parallelism();

        let mut event_manager = EventManager::new(thread_count);
        event_manager.assign_thread_event_buffer(0);

        let mut frame_buffer_manager = FrameBufferManager::new(thread_count);
        frame_buffer_manager.assign_thread_frame_buffer(0);

        let input = GameInput::new(window.inner_size());

        #[cfg(target_vendor = "apple")]
        let graphics = Metal::new(window);

        #[cfg(not(target_vendor = "apple"))]
        let graphics = Vulkan::new(window);

        Self {
            event_manager,
            fixed_update: FixedUpdate::new(thread_count),
            frame_buffer_manager,
            frame_update_systems: FrameUpdateSystems::new(),
            game_controller: GameController,
            input,
            last_fixed_update_instant: Instant::now(),
            task_executor: TaskExecutor,
            graphics,
        }
    }

    pub fn handle_input(&mut self, event: WindowEvent) {
        // writes to previous frame event buffers
        self.input.handle_input(event);
    }

    pub fn frame(&mut self) {
        self.update_fixed();

        self.update_game_state();

        self.event_manager.swap();
        self.frame_buffer_manager.swap();

        self.update_and_render_frame();
    }

    fn update_fixed(&mut self) {
        let now = Instant::now();

        while now.duration_since(self.last_fixed_update_instant) >= FIXED_TIMESTEP {
            self.last_fixed_update_instant += FIXED_TIMESTEP;

            let await_task = self.fixed_update.await_prev_update();
            self.task_executor.execute_blocking(await_task);

            // if last iteration, swap with frame updates
            if now.duration_since(self.last_fixed_update_instant) < FIXED_TIMESTEP {
                let swap_task = self.fixed_update.swap(&mut self.frame_update_systems);
                self.task_executor.execute_blocking(swap_task);
            }

            self.fixed_update.execute(&mut self.task_executor);
        }
    }

    fn update_game_state(&mut self) {
        let event_delegate = self.event_manager.borrow();

        self.input.update(&event_delegate);
        self.game_controller.update(&event_delegate);
    }

    fn update_and_render_frame(&mut self) {
        let input_interface = GameInputInterface::new(&self.input);
        let frame_buffer_delegate = self.frame_buffer_manager.delegate();
        let frame_buffer_reader = frame_buffer_delegate.reader();
        let frame_buffer_writer = frame_buffer_delegate.writer();
        let event_delegate = self.event_manager.borrow();

        let frame_task = async {
            let frame_update_task = self.frame_update_systems.update(
                &event_delegate,
                &frame_buffer_writer,
                input_interface,
            );

            let graphics_task = self.graphics.frame(&frame_buffer_reader);

            pin_mut!(frame_update_task);
            pin_mut!(graphics_task);

            parallel([frame_update_task, graphics_task]).await;
        };

        self.task_executor.execute_blocking(frame_task);
    }
}
