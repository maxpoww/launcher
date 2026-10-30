// ioprobe <dir> <write-GiB>: what a big write does to the rest of the machine. One thread writes GiB of data
// (buffered, then fsync); meanwhile a small file is written and fsynced every 100 ms (what a browser's database does)
// and a page of an old file is read with O_DIRECT every 20 ms. Latencies before, during, and the writer's time.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
static volatile int stop=0; static char dir[512]; static double gib;
#define N 200000
static double fs[N],rd[N]; static int nfs,nrd;
static void*fsyncer(void*x){char p[600];snprintf(p,600,"%s/iop.small",dir);int fd=open(p,O_CREAT|O_WRONLY,0600);char b[4096];memset(b,1,4096);while(!stop){double t=now();pwrite(fd,b,4096,0);fsync(fd);if(nfs<N)fs[nfs++]=now()-t;usleep(100000);}close(fd);unlink(p);return 0;}
static void*reader(void*x){char p[600];snprintf(p,600,"%s/iop.read",dir);int fd=open(p,O_RDONLY|O_DIRECT);void*b;posix_memalign(&b,4096,4096);uint64_t r=88172645463325252ULL;off_t sz=lseek(fd,0,SEEK_END)/4096;while(!stop){r^=r<<13;r^=r>>7;r^=r<<17;double t=now();pread(fd,b,4096,(r%sz)*4096);if(nrd<N)rd[nrd++]=now()-t;usleep(20000);}close(fd);return 0;}
static int cmp(const void*a,const void*b){double x=*(double*)a,y=*(double*)b;return x<y?-1:x>y;}
static void rep(const char*n,double*v,int c){qsort(v,c,sizeof(double),cmp);if(!c)return;printf("%-14s n=%5d median %8.2f ms  p95 %8.2f  p99 %8.2f  max %8.2f\n",n,c,v[c/2]*1e3,v[c*95/100]*1e3,v[c*99/100]*1e3,v[c-1]*1e3);}
int main(int argc,char**argv){strncpy(dir,argv[1],500);gib=atof(argv[2]);char p[600];snprintf(p,600,"%s/iop.read",dir);
  {int fd=open(p,O_CREAT|O_WRONLY|O_TRUNC,0600);char*b=malloc(1<<20);for(int i=0;i<1<<20;i++)b[i]=rand();for(int i=0;i<1024;i++)write(fd,b,1<<20);fsync(fd);close(fd);posix_fadvise(open(p,O_RDONLY),0,0,POSIX_FADV_DONTNEED);}
  pthread_t a,c;pthread_create(&a,0,fsyncer,0);pthread_create(&c,0,reader,0);
  sleep(5);stop=1;pthread_join(a,0);pthread_join(c,0);rep("quiet fsync",fs,nfs);rep("quiet read",rd,nrd);nfs=nrd=0;stop=0;
  pthread_create(&a,0,fsyncer,0);pthread_create(&c,0,reader,0);
  char w[600];snprintf(w,600,"%s/iop.big",dir);int fd=open(w,O_CREAT|O_WRONLY|O_TRUNC,0600);char*b=malloc(1<<20);for(int i=0;i<1<<20;i++)b[i]=rand();
  double t0=now();double worst=0;for(long i=0;i<(long)(gib*1024);i++){double t=now();write(fd,b,1<<20);double d=now()-t;if(d>worst)worst=d;}double t1=now();fsync(fd);double t2=now();close(fd);
  stop=1;pthread_join(a,0);pthread_join(c,0);printf("writer %.1f GiB: buffered %.2f s, then fsync %.2f s, total %.2f s, worst 1 MiB write %.1f ms\n",gib,t1-t0,t2-t1,t2-t0,worst*1e3);
  rep("loaded fsync",fs,nfs);rep("loaded read",rd,nrd);unlink(w);unlink(p);return 0;}
