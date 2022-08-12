use crate::API_DATA_RECEPTION_CHANNEL_SIZE;

/// An alias for `Box<T>`.
pub type Dyn<T> = std::boxed::Box<T>;
/// An immutable callback function.
pub type Callback<'a, T> = Dyn<dyn Fn(T) + Send + Sync + 'a>;

/// A type that can be converted into a [`Callback`]-receiver pair.
///
/// When Zenoh functions accept types that implement these, it intends to use the [`Callback`] as just that,
/// while granting you access to the receiver through the returned value via [`std::ops::Deref`] and [`std::ops::DerefMut`].
///
/// Any closure that accepts `T` can be converted into a pair of itself and `()`.
pub trait IntoCallbackReceiverPair<'a, T> {
    type Receiver;
    fn into_cb_receiver_pair(self) -> (Callback<'a, T>, Self::Receiver);
}
impl<'a, T, F> IntoCallbackReceiverPair<'a, T> for F
where
    F: Fn(T) + Send + Sync + 'a,
{
    type Receiver = ();
    fn into_cb_receiver_pair(self) -> (Callback<'a, T>, Self::Receiver) {
        (Box::from(self), ())
    }
}
impl<T: Send + 'static> IntoCallbackReceiverPair<'static, T>
    for (flume::Sender<T>, flume::Receiver<T>)
{
    type Receiver = flume::Receiver<T>;

    fn into_cb_receiver_pair(self) -> (Callback<'static, T>, Self::Receiver) {
        let (sender, receiver) = self;
        (
            Box::new(move |t| {
                if let Err(e) = sender.send(t) {
                    log::error!("{}", e)
                }
            }),
            receiver,
        )
    }
}
pub struct DefaultHandler;
impl<T: Send + 'static> IntoCallbackReceiverPair<'static, T> for DefaultHandler {
    type Receiver = flume::Receiver<T>;
    fn into_cb_receiver_pair(self) -> (Callback<'static, T>, Self::Receiver) {
        flume::bounded(*API_DATA_RECEPTION_CHANNEL_SIZE).into_cb_receiver_pair()
    }
}
impl<T: Send + Sync + 'static> IntoCallbackReceiverPair<'static, T>
    for (std::sync::mpsc::SyncSender<T>, std::sync::mpsc::Receiver<T>)
{
    type Receiver = std::sync::mpsc::Receiver<T>;
    fn into_cb_receiver_pair(self) -> (Callback<'static, T>, Self::Receiver) {
        let (sender, receiver) = self;
        (
            Box::new(move |t| {
                if let Err(e) = sender.send(t) {
                    log::error!("{}", e)
                }
            }),
            receiver,
        )
    }
}

/// A function that can transform a [`FnMut`]`(T)` to
/// a [`Fn`]`(T)` with the help of a [`Mutex`](std::sync::Mutex).
pub fn locked<T>(fnmut: impl FnMut(T)) -> impl Fn(T) {
    let lock = std::sync::Mutex::new(fnmut);
    move |x| zlock!(lock)(x)
}