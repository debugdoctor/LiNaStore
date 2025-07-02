import java.net.*;

public class LiNaStoreClient {
    private Socket socket;

    public LiNaStoreClient(String ip, int port){
        try{
            socket = new Socket(ip, port);
        } catch (IOException e) {
            throw e;
        }

    }
    public static void main(){

    }
}